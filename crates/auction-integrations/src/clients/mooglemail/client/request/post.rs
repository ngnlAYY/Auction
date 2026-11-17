//! MoogleMail typed POST：写入前预检、写入围栏与远端结果分类；结果无法证明时一律写入未知。与 GET
//! 同样分两段：排队（请求门、在途锁、账号忙与 owner 复核、写屏障）在调用方 future 内，被放弃即
//! 撤票、不发出；越过发出边界后调用方放弃等待只丢弃结果，这一次物理请求交给独立任务走完，在途锁
//! 不随调用方提前释放。被放弃的写入不补发，也不续做调用方流程的后续步骤。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use reqwest::{StatusCode, header};

use super::super::reads::WriteBarrier;
use super::super::security::{
    is_html_content_type, is_session_expired, is_state_lock_limited, post_allowed,
};
use super::super::{MoogleMailClient, USER_AGENT};
use super::body::{DiagnosticResponse, SensitiveBytes};
use crate::clients::gate::GateClass;
use crate::clients::metrics::{MailGateWaitGuard, MailRequestGuard, mail_result, mail_signal};
use crate::clients::mooglemail::protocol::MoogleMailCookie;
use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailForm, MoogleMailOperation, MoogleMailTarget,
};
use crate::clients::operation_lock::OperationLockGuard;
use crate::clients::sent_request::SentRequest;
use crate::safe_diagnostics;

/// 排到发出边界的写入：已取得请求门令牌与在途锁，账号忙与 owner 已复核，写屏障已建立，请求已
/// 构造但尚未发出。丢弃即放弃发出并释放在途锁，写屏障随之执行第二道失效。
struct QueuedWrite {
    request: reqwest::Request,
    operation: MoogleMailOperation,
    in_flight: OperationLockGuard,
    write_barrier: WriteBarrier,
}

impl MoogleMailClient {
    /// 对 compose/detail/Armory organize typed target 执行恰好一次表单 POST。
    /// 请求开始后，无论是网络错误、响应结构错误还是远端明确拒绝，都不能证明远端没有
    /// 发生变化，因此统一返回 write_unknown，由上层进入人工对账或基于页面证明继续。
    ///
    /// 排队在调用方 future 内，被放弃即撤票、不发出。发出后调用方放弃等待（投递 `request_timeout`、
    /// 租约失效、外层预算、任务取消）只丢弃结果：这一次请求交给独立任务走完，在途锁持有到远端回应
    /// 或客户端超时，紧接的同账号请求不与远端仍在处理的写入重叠。放弃的调用方只能按写入未知收口，
    /// 绝不重发；客户端超时同样不能证明远端未执行。
    pub async fn post(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
        form: MoogleMailForm,
    ) -> Result<Vec<u8>, MoogleMailError> {
        let queued = match self.queue_write(target, cookie, form).await {
            Ok(queued) => queued,
            Err(error) => {
                return Err(self
                    .report_write_failure(target, &DiagnosticResponse::default(), error)
                    .await);
            }
        };
        let client = self.clone();
        let target = target.clone();
        // 排队结束与发出之间没有等待点，放弃只会发生在越过发出边界之后。剩余部分只走完这一次物理
        // 请求与失败留证；被放弃的正文无人接收，以敏感缓冲交接，随 Drop 清零。
        SentRequest::new(async move {
            let mut response = DiagnosticResponse::default();
            match client.send_write(queued, &target, &mut response).await {
                Ok(body) => Ok(SensitiveBytes(body)),
                Err(error) => Err(client.report_write_failure(&target, &response, error).await),
            }
        })
        .await
        .map(|mut body| std::mem::take(&mut body.0))
    }

    /// 写入失败的统一收尾：按需留存加密证据，再记一条脱敏诊断（世界取自错误投影）。
    async fn report_write_failure(
        &self,
        target: &MoogleMailTarget,
        response: &DiagnosticResponse,
        error: MoogleMailError,
    ) -> MoogleMailError {
        let error =
            self.diagnose_http(target, response.status, response.body.as_deref(), error).await;
        safe_diagnostics::warn(
            "mooglemail_send",
            "write_transaction",
            safe_diagnostics::project(&error),
            DiagnosticContext::default(),
        );
        error
    }

    /// 发出前的排队阶段：入口预检、请求门、在途锁、账号忙复核、owner 复核与写屏障，止于发出边界。
    /// 全程在调用方 future 内：被丢弃时请求门票据与在途锁登记同步撤销，已取得的在途锁随之释放，
    /// 不会在无人等待时补发。
    async fn queue_write(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
        form: MoogleMailForm,
    ) -> Result<QueuedWrite, MoogleMailError> {
        let operation = form.operation();
        if target.origin != self.builder.origin || !post_allowed(target.operation, operation) {
            return Err(self.configuration_for(operation, target.world));
        }
        // 账号忙期间写入一律不发网络，并且必须在 sending 围栏与请求发出之前失败。
        if !self.account_busy.permits_write(target.world, std::time::Instant::now()) {
            return Err(self.account_busy_error(operation, target.world));
        }
        let gate_wait = MailGateWaitGuard::start(
            std::sync::Arc::clone(&self.external_metrics),
            target.world,
            operation,
        );
        // 表单写入一律前台：账号忙与发送围栏都不能被后台读取挤到后面。
        let gate_result = self.gate.wait(GateClass::Foreground).await;
        gate_wait.finish(if gate_result.is_ok() {
            auction_application::external_metrics::WaitOutcome::Acquired
        } else {
            auction_application::external_metrics::WaitOutcome::Error
        });
        gate_result.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_send",
                "wait_rate",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(target.world.as_str()),
                    write_started: Some(false),
                    ..Default::default()
                },
            );

            MoogleMailError::new(
                operation,
                target.world,
                MoogleMailErrorCategory::RemoteTemporary,
                true,
            )
        })?;
        // 与读取共用同一在途锁：写入不与任何物理请求重叠，避免远端状态锁限流。前台登记插到
        // 所有已排队的后台读取之前，不再等它们逐个读完正文；同档仍按登记顺序。
        let in_flight = self.in_flight.lock(GateClass::Foreground).await;
        // 排队期间在途读取可能已打开账号忙窗口；复核仍在写入围栏与请求发出之前，不是写入未知。
        if !self.account_busy.permits_write(target.world, std::time::Instant::now()) {
            return Err(self.account_busy_error(operation, target.world));
        }

        let mut cookie_header =
            header::HeaderValue::from_str(cookie.expose()).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "encode_cookie",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(target.world.as_str()),
                        write_started: Some(false),
                        ..Default::default()
                    },
                );
                self.configuration_for(operation, target.world)
            })?;
        cookie_header.set_sensitive(true);
        let request = self
            .client
            .post(target.url.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie_header)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .timeout(self.request_timeout)
            .body(form.encode())
            .build()
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "build_request",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(target.world.as_str()),
                        write_started: Some(false),
                        ..Default::default()
                    },
                );
                self.configuration_for(operation, target.world)
            })?;
        // 先前步骤可能已准备装备或暂存 compose 资产；失锁不能被降级为可安全重发。
        self.check_ownership().await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_send",
                "check_request_owner",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(target.world.as_str()),
                    write_started: Some(true),
                    ..Default::default()
                },
            );
            self.write_unknown(operation, target.world)
        })?;
        if !cookie.is_active() {
            return Err(self.write_unknown(operation, target.world));
        }
        // 必须覆盖请求发出前至完整响应；Drop 同时处理取消和未知远端结果。发出后被放弃时随剩余部分
        // 持有到物理请求结束。
        let write_barrier =
            self.write_barrier(cookie, target.world).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "persist_write_fence",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(target.world.as_str()),
                        write_started: Some(true),
                        ..Default::default()
                    },
                );
                self.write_unknown(operation, target.world)
            })?;
        Ok(QueuedWrite { request, operation, in_flight, write_barrier })
    }

    /// 发出写入并读完回应、分类结果；在途锁持有到正文读完（或客户端超时），写屏障持有到分类结束。
    /// 剩余部分有界：HTTP 交换受整次请求超时约束。调用方放弃等待时由独立任务走完，见
    /// [`SentRequest`]。
    async fn send_write(
        &self,
        queued: QueuedWrite,
        target: &MoogleMailTarget,
        diagnostic: &mut DiagnosticResponse,
    ) -> Result<Vec<u8>, MoogleMailError> {
        let QueuedWrite { request, operation, in_flight, write_barrier: _write_barrier } = queued;
        let request_metrics = MailRequestGuard::start(
            std::sync::Arc::clone(&self.external_metrics),
            target.world,
            operation,
        );
        let result = async {
            let response = self.client.execute(request).await.map_err(|diagnostic_cause| {
                if diagnostic_cause.is_timeout() {
                    mail_signal(
                        self.external_metrics.as_ref(),
                        target.world,
                        operation,
                        auction_application::external_metrics::MailSignal::Timeout,
                    );
                }
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "submit_request",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(target.world.as_str()),
                        write_started: Some(true),
                        ..Default::default()
                    },
                );
                self.write_unknown(operation, target.world)
            })?;
            diagnostic.status = Some(response.status().as_u16());

            if response.url().as_str() != target.url.as_str() {
                return Err(self.write_unknown(operation, target.world));
            }
            if response
                .headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    !value.trim().is_empty() && !value.eq_ignore_ascii_case("identity")
                })
                || response
                    .content_length()
                    .is_some_and(|length| length > self.maximum_response_bytes as u64)
            {
                return Err(self.write_unknown(operation, target.world));
            }
            let status = response.status();
            let location = response.headers().get(header::LOCATION).cloned();
            let is_html = is_html_content_type(response.headers());
            diagnostic.body =
                Some(self.read_response_body(response, operation, target.world, true).await?);
            drop(in_flight);
            let body = diagnostic
                .body
                .as_deref()
                .ok_or_else(|| self.write_unknown(operation, target.world))?;
            // 状态锁限流正文不能证明远端未执行写入：只单独计数留痕，结果分类仍交给调用方的
            // 页面证明，无法证明时保持写入未知，绝不自动重发。
            let state_lock_limited = is_state_lock_limited(body);
            if state_lock_limited {
                mail_signal(
                    self.external_metrics.as_ref(),
                    target.world,
                    operation,
                    auction_application::external_metrics::MailSignal::RateLimited,
                );
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "state_lock_limited",
                    safe_diagnostics::category(
                        auction_application::core::diagnostics::SafeErrorKind::REMOTE_RATE_LIMITED,
                    ),
                    DiagnosticContext {
                        world: diagnostic_world(target.world.as_str()),
                        write_started: Some(true),
                        ..Default::default()
                    },
                );
            }
            if is_session_expired(status, location.as_ref(), body) {
                return Err(MoogleMailError::after_write(
                    operation,
                    target.world,
                    MoogleMailErrorCategory::SessionExpired,
                ));
            }
            if status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error()
                || !status.is_success()
                || !is_html
            {
                // 同一响应的状态锁限流正文已计数，429 不再重复计一次。
                if status == StatusCode::TOO_MANY_REQUESTS && !state_lock_limited {
                    mail_signal(
                        self.external_metrics.as_ref(),
                        target.world,
                        operation,
                        auction_application::external_metrics::MailSignal::RateLimited,
                    );
                }
                return Err(self.write_unknown(operation, target.world));
            }
            diagnostic.body.take().ok_or_else(|| self.write_unknown(operation, target.world))
        }
        .await;
        request_metrics.finish(mail_result(&result));
        result
    }
}
