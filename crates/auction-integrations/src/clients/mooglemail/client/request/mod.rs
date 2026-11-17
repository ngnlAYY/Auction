//! MoogleMail typed GET/POST 请求与远端结果分类。GET 分两段：排队在调用方 future 内，被放弃即
//! 撤票；发出后被放弃时剩余部分交给独立任务走完，在途锁不随调用方提前释放。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use reqwest::{StatusCode, header};

use super::security::{
    is_hentaiverse_battle_response, is_html_content_type, is_javascript_content_type,
    is_session_expired, is_state_lock_limited,
};
use super::{MoogleMailClient, USER_AGENT};
use crate::clients::gate::GateClass;
use crate::clients::metrics::{MailGateWaitGuard, MailRequestGuard, mail_result, mail_signal};
use crate::clients::mooglemail::protocol::MoogleMailCookie;
use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailTarget,
    MoogleMailWorld,
};
use crate::clients::operation_lock::OperationLockGuard;
use crate::clients::sent_request::SentRequest;
use crate::safe_diagnostics;

mod body;
mod equipment;
mod pending;
mod post;
#[cfg(test)]
mod tests;

use body::DiagnosticResponse;
pub(super) use body::SensitiveBytes;
pub(crate) use equipment::{EquipmentReadPermit, equipment_read_hold};
pub(crate) use pending::PendingRead;

/// 排到发出边界的读取：已取得请求门令牌与在途锁，账号忙与 owner 已复核，请求已构造但尚未发出。
/// 丢弃即放弃发出并释放在途锁。
struct QueuedRead {
    request: reqwest::Request,
    in_flight: OperationLockGuard,
    probes_account: bool,
}

impl MoogleMailClient {
    /// 对 typed target 执行一次受限 GET。
    /// 正文不进入错误对象或普通日志；异常响应只允许交给独立加密证据记录器。
    /// Cookie 仅在建立请求时短暂复制到 HTTP header，不参与响应留证。
    ///
    /// 排队在调用方 future 内，被放弃即撤票、不补发。发出后调用方放弃等待（外层读取预算、item
    /// timeout、任务取消）只丢弃结果：剩余部分交给独立任务走完，在途锁持有到远端回应或客户端超时，
    /// 紧接的同账号请求不与远端仍在处理的请求重叠。客户端超时不能证明远端已停止处理。
    pub async fn get(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
    ) -> Result<Vec<u8>, MoogleMailError> {
        // 详情 GET 会改变 Inbox/Read Mail 成员关系：写锁随发出阶段持有到物理请求结束。
        let membership = if target.operation == MoogleMailOperation::Detail {
            Some(self.incoming_detail_guard(cookie, target.world).await?)
        } else {
            None
        };
        let pending = self.queue_pending(target.clone(), cookie).await?;
        // 排队结束与发出之间没有等待点，放弃只会发生在越过发出边界之后。被放弃的正文无人接收，
        // 以敏感缓冲交接，随 Drop 清零。
        SentRequest::new(async move {
            let _membership = membership;
            pending.send().await.map(SensitiveBytes)
        })
        .await
        .map(|mut body| std::mem::take(&mut body.0))
    }

    /// 读取失败的统一收尾：按需留存加密证据，再记一条脱敏诊断。
    async fn report_read_failure(
        &self,
        target: &MoogleMailTarget,
        response: &DiagnosticResponse,
        error: MoogleMailError,
    ) -> MoogleMailError {
        let error =
            self.diagnose_http(target, response.status, response.body.as_deref(), error).await;
        let projection = safe_diagnostics::project(&error);
        // 被放弃后在独立任务里收尾时没有调用方的诊断上下文，世界显式带上。
        let context = DiagnosticContext {
            world: diagnostic_world(target.world.as_str()),
            ..Default::default()
        };
        // 账号忙是正常的退避信号，不应在运维告警中表现为 WARN；其他分类仍保留 WARN。
        if error.category.as_str() == "account_busy" {
            safe_diagnostics::info("mooglemail_read", "read_transaction", projection, context);
        } else {
            safe_diagnostics::warn("mooglemail_read", "read_transaction", projection, context);
        }
        error
    }

    /// 发出前的排队阶段：账号忙预检、请求门、在途锁、账号忙复核与 owner 复核，止于发出边界。
    /// 全程在调用方 future 内：被丢弃时请求门票据与在途锁登记同步撤销，已取得的在途锁随之释放，
    /// 不会在无人等待时补发。owner 探测按自身设计在独立任务中有界完成，不发出邮件请求。
    async fn queue_read(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
    ) -> Result<QueuedRead, MoogleMailError> {
        if target.origin != self.builder.origin {
            return Err(self.configuration(target));
        }
        // 不能充当账号探测的读取（候选凭据检查）与装备页同口径：窗口内一律不发网络，也不
        // 占用窗口唯一的探测额度；常规读取照旧可作探测。
        let probes_account = target.operation.probes_account();
        // 已知的熔断拒绝先于占用限速令牌；预检不占用探测额度，额度在取得在途锁后才占用。
        let may_queue = if probes_account {
            self.account_busy.may_queue_read(target.world, std::time::Instant::now())
        } else {
            self.account_busy.permits_unprobed_read(target.world, std::time::Instant::now())
        };
        if !may_queue {
            return Err(self.account_busy_error(target.operation, target.world));
        }
        let gate_wait = MailGateWaitGuard::start(
            std::sync::Arc::clone(&self.external_metrics),
            target.world,
            target.operation,
        );
        // 纯只读观测使用后台档；compose/detail 等写入流程页面仍有前台份额。
        let class = if target.operation.is_background_read() {
            GateClass::Background
        } else {
            GateClass::Foreground
        };
        let gate_result = self.gate.wait(class).await;
        gate_wait.finish(if gate_result.is_ok() {
            auction_application::external_metrics::WaitOutcome::Acquired
        } else {
            auction_application::external_metrics::WaitOutcome::Error
        });
        gate_result.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "wait_rate",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(target.world.as_str()),
                    ..Default::default()
                },
            );

            MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::RemoteTemporary,
                true,
            )
        })?;
        // 门不等待前一请求完成：慢响应期间已取得令牌的请求在在途锁上排队。按同一档位排队，
        // 前台读取插到已排队的后台读取之前；owner 复核在取得锁之后，贴近真实发出时刻。
        let in_flight = self.in_flight.lock(class).await;
        // 排队期间前一请求可能已打开账号忙窗口：发出网络前复核，并在此才占用唯一的探测额度。
        let permitted = if probes_account {
            self.account_busy.permits_read(target.world, std::time::Instant::now())
        } else {
            self.account_busy.permits_unprobed_read(target.world, std::time::Instant::now())
        };
        if !permitted {
            return Err(self.account_busy_error(target.operation, target.world));
        }

        let mut cookie_header =
            header::HeaderValue::from_str(cookie.expose()).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "encode_cookie",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(target.world.as_str()),
                        ..Default::default()
                    },
                );
                self.configuration(target)
            })?;
        cookie_header.set_sensitive(true);
        let request = self
            .client
            .get(target.url.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie_header)
            .timeout(self.request_timeout)
            .build()
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "build_request",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(target.world.as_str()),
                        ..Default::default()
                    },
                );
                self.configuration(target)
            })?;
        // 限速等待后、真正发出请求前重新证明原 session 仍持有凭据 owner。
        self.check_ownership().await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "check_request_owner",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(target.world.as_str()),
                    ..Default::default()
                },
            );

            MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::CredentialUnavailable,
                false,
            )
        })?;
        if !cookie.is_active() {
            return Err(MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::CredentialUnavailable,
                false,
            ));
        }
        Ok(QueuedRead { request, in_flight, probes_account })
    }

    /// 发出请求、读完正文并分类结果；在途锁持有到正文读完（或客户端超时）且账号忙分类完成。调用方
    /// 放弃等待时由独立任务走完，见 [`SentRequest`]。
    async fn send_read(
        &self,
        queued: QueuedRead,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
        diagnostic: &mut DiagnosticResponse,
    ) -> Result<Vec<u8>, MoogleMailError> {
        let QueuedRead { request, in_flight, probes_account } = queued;
        let request_metrics = MailRequestGuard::start(
            std::sync::Arc::clone(&self.external_metrics),
            target.world,
            target.operation,
        );
        let result = async {
            let response = self.client.execute(request).await.map_err(|diagnostic_cause| {
                if diagnostic_cause.is_timeout() {
                    mail_signal(
                        self.external_metrics.as_ref(),
                        target.world,
                        target.operation,
                        auction_application::external_metrics::MailSignal::Timeout,
                    );
                }
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "fetch_page",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(target.world.as_str()),
                        ..Default::default()
                    },
                );

                MoogleMailError::new(
                    target.operation,
                    target.world,
                    MoogleMailErrorCategory::RemoteTemporary,
                    true,
                )
            })?;
            diagnostic.status = Some(response.status().as_u16());

            if response.url().as_str() != target.url.as_str() {
                return Err(self.protocol(target));
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
                return Err(self.response_too_large(target));
            }
            let status = response.status();
            let location = response.headers().get(header::LOCATION).cloned();
            let is_html = is_html_content_type(response.headers());
            let is_javascript =
                target.inventory_metadata && is_javascript_content_type(response.headers());

            diagnostic.body = Some(
                self.read_response_body(response, target.operation, target.world, false).await?,
            );
            let body = diagnostic.body.as_deref().ok_or_else(|| self.protocol(target))?;

            // 名称脚本不是 HTML；避免把装备名中的登录或战斗文字当作页面状态。
            if !is_javascript && is_session_expired(status, location.as_ref(), body) {
                return Err(MoogleMailError::new(
                    target.operation,
                    target.world,
                    MoogleMailErrorCategory::SessionExpired,
                    false,
                ));
            }
            if !is_javascript && is_hentaiverse_battle_response(body) {
                mail_signal(
                    self.external_metrics.as_ref(),
                    target.world,
                    target.operation,
                    auction_application::external_metrics::MailSignal::AccountBusy,
                );
                // 熔断只升级请求退避；凭据仍有效，业务结果仍由调用方按临时错误处理。
                // 在途锁随本异步块结束才释放，排队请求取得锁后的复核一定能看到这个窗口。
                self.account_busy.record_busy(target.world, std::time::Instant::now());
                return Err(self.account_busy_error(target.operation, target.world));
            }
            // 物理请求已结束且熔断状态已按本次响应更新；后续分类与 owner 复核不占用在途锁。
            drop(in_flight);
            // HV 同一账号请求重叠时返回状态锁限流正文；只读请求按临时限流处理，不交给解析器
            // 误判为协议漂移。
            if status == StatusCode::TOO_MANY_REQUESTS || is_state_lock_limited(body) {
                mail_signal(
                    self.external_metrics.as_ref(),
                    target.world,
                    target.operation,
                    auction_application::external_metrics::MailSignal::RateLimited,
                );
                return Err(MoogleMailError::new(
                    target.operation,
                    target.world,
                    MoogleMailErrorCategory::RateLimited,
                    true,
                ));
            }
            if status.is_server_error() {
                return Err(MoogleMailError::new(
                    target.operation,
                    target.world,
                    MoogleMailErrorCategory::RemoteTemporary,
                    true,
                ));
            }
            if target.inventory_metadata {
                let script = body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(body);
                if status != StatusCode::OK
                    || !is_javascript
                    || script
                        .iter()
                        .find(|byte| !byte.is_ascii_whitespace())
                        .is_none_or(|byte| *byte == b'<')
                {
                    return Err(self.protocol(target));
                }
            } else if !status.is_success() || !is_html {
                return Err(self.protocol(target));
            }
            self.check_ownership().await.map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "check_response_owner",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(target.world.as_str()),
                        ..Default::default()
                    },
                );

                MoogleMailError::new(
                    target.operation,
                    target.world,
                    MoogleMailErrorCategory::CredentialUnavailable,
                    false,
                )
            })?;
            if !cookie.is_active() {
                return Err(MoogleMailError::new(
                    target.operation,
                    target.world,
                    MoogleMailErrorCategory::CredentialUnavailable,
                    false,
                ));
            }
            // 只有能代表当前账号的读取才结束忙碌期；候选凭据可能属于另一账号，成功不能证明
            // 当前账号已退出战斗。
            if probes_account {
                self.account_busy.record_success(target.world);
            }
            diagnostic.body.take().ok_or_else(|| self.protocol(target))
        }
        .await;
        request_metrics.finish(mail_result(&result));
        result
    }

    pub(super) fn configuration(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::Configuration,
            false,
        )
    }

    pub(super) fn configuration_for(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::new(operation, world, MoogleMailErrorCategory::Configuration, false)
    }

    /// 账号忙是临时限流分类：不表示 Cookie 失效，也不表示写入已经发生。
    pub(super) fn account_busy_error(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::new(operation, world, MoogleMailErrorCategory::AccountBusy, true)
    }

    pub(super) fn write_unknown(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::after_write(operation, world, MoogleMailErrorCategory::WriteUnknown)
    }

    pub(super) fn protocol(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::ProtocolDrift,
            false,
        )
    }

    pub(super) fn response_too_large(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::ResponseTooLarge,
            false,
        )
    }
}
