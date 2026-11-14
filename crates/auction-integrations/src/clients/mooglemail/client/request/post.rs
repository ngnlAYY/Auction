//! MoogleMail typed POST：写入前预检、写入围栏与远端结果分类；结果无法证明时一律写入未知。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use reqwest::{StatusCode, header};

use super::super::security::{is_html_content_type, is_session_expired, post_allowed};
use super::super::{
    MoogleMailClient, MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailForm,
    MoogleMailTarget, USER_AGENT,
};
use super::DiagnosticResponse;
use crate::clients::metrics::{MailGateWaitGuard, MailRequestGuard, mail_result, mail_signal};
use crate::safe_diagnostics;

impl MoogleMailClient {
    /// 对 compose/detail/Armory organize typed target 执行恰好一次表单 POST。
    /// 请求开始后，无论是网络错误、响应结构错误还是远端明确拒绝，都不能证明远端没有
    /// 发生变化，因此统一返回 write_unknown，由上层进入人工对账或基于页面证明继续。
    pub async fn post(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
        form: MoogleMailForm,
    ) -> Result<Vec<u8>, MoogleMailError> {
        let mut response = DiagnosticResponse::default();
        match self.post_inner(target, cookie, form, &mut response).await {
            Ok(body) => Ok(body),
            Err(error) => {
                let error = self
                    .diagnose_http(target, response.status, response.body.as_deref(), error)
                    .await;
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "write_transaction",
                    safe_diagnostics::project(&error),
                    DiagnosticContext::default(),
                );
                Err(error)
            }
        }
    }

    async fn post_inner(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
        form: MoogleMailForm,
        diagnostic: &mut DiagnosticResponse,
    ) -> Result<Vec<u8>, MoogleMailError> {
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
        let gate_result = self.gate.wait(auction_application::mail::GateClass::Foreground).await;
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
        // 必须覆盖请求发出前至完整响应；Drop 同时处理取消和未知远端结果。
        let _write_barrier =
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
            let body = diagnostic
                .body
                .as_deref()
                .ok_or_else(|| self.write_unknown(operation, target.world))?;
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
                if status == StatusCode::TOO_MANY_REQUESTS {
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
