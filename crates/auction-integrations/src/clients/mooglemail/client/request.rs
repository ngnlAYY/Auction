//! MoogleMail typed GET/POST 请求与远端结果分类。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::clients::metrics::{MailGateWaitGuard, MailRequestGuard, mail_result, mail_signal};
use crate::diagnostics as safe_diagnostics;

use super::security::{
    is_hentaiverse_battle_response, is_html_content_type, is_javascript_content_type,
    is_session_expired, post_allowed,
};
use super::*;

impl MoogleMailClient {
    /// 对 typed target 执行一次受限 GET。
    /// 正文不进入错误对象或普通日志；异常响应只允许交给独立加密证据记录器。
    /// Cookie 仅在建立请求时短暂复制到 HTTP header，不参与响应留证。
    pub async fn get(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
    ) -> Result<Vec<u8>, MoogleMailError> {
        let _membership = if target.operation == MoogleMailOperation::Detail {
            Some(self.incoming_detail_guard(cookie, target.world).await?)
        } else {
            None
        };
        let mut response = DiagnosticResponse::default();
        match self.get_inner(target, cookie, &mut response).await {
            Ok(body) => Ok(body),
            Err(error) => {
                let error = self
                    .diagnose_http(target, response.status, response.body.as_deref(), error)
                    .await;
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "read_transaction",
                    safe_diagnostics::project(&error),
                    DiagnosticContext::default(),
                );
                Err(error)
            }
        }
    }

    async fn get_inner(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
        diagnostic: &mut DiagnosticResponse,
    ) -> Result<Vec<u8>, MoogleMailError> {
        if target.origin != self.builder.origin {
            return Err(self.configuration(target));
        }
        // 熔断窗口内只放行一次只读探测；其余读取在占用限速令牌和发出网络之前失败。
        if !self.account_busy.permits_read(target.world, std::time::Instant::now()) {
            return Err(self.account_busy_error(target.operation, target.world));
        }
        let gate_wait = MailGateWaitGuard::start(
            std::sync::Arc::clone(&self.external_metrics),
            target.world,
            target.operation,
        );
        // 纯只读观测使用后台档；compose/detail 等写入流程页面仍有前台份额。
        let class = if target.operation.is_background_read() {
            auction_application::mail::GateClass::Background
        } else {
            auction_application::mail::GateClass::Foreground
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
                self.account_busy.record_busy(target.world, std::time::Instant::now());
                return Err(self.account_busy_error(target.operation, target.world));
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
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
            // 完整读取成功证明账号已退出战斗：结束当前忙碌期并重置退避档位。
            self.account_busy.record_success(target.world);
            diagnostic.body.take().ok_or_else(|| self.protocol(target))
        }
        .await;
        request_metrics.finish(mail_result(&result));
        result
    }

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

    /// 每块正文入缓冲前校验总量；无 Content-Length 的响应也不能绕过内存边界。
    async fn read_response_body(
        &self,
        mut response: reqwest::Response,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
        write_started: bool,
    ) -> Result<Vec<u8>, MoogleMailError> {
        let read_error = || {
            if write_started {
                self.write_unknown(operation, world)
            } else {
                MoogleMailError::new(
                    operation,
                    world,
                    MoogleMailErrorCategory::RemoteTemporary,
                    true,
                )
            }
        };
        let capacity = response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default()
            .min(self.maximum_response_bytes);
        time::timeout(self.request_timeout, async {
            // 有界预留减少敏感正文扩容复制；失败或取消清理半截正文，诊断仅保留完整响应。
            let mut body = SensitiveBytes(Vec::with_capacity(capacity));
            while let Some(chunk) = response.chunk().await.map_err(|diagnostic_cause| {
                if diagnostic_cause.is_timeout() {
                    mail_signal(
                        self.external_metrics.as_ref(),
                        world,
                        operation,
                        auction_application::external_metrics::MailSignal::Timeout,
                    );
                }
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "read_response_chunk",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        write_started: Some(write_started),
                        ..Default::default()
                    },
                );
                read_error()
            })? {
                if body.0.len().saturating_add(chunk.len()) > self.maximum_response_bytes {
                    return Err(if write_started {
                        self.write_unknown(operation, world)
                    } else {
                        MoogleMailError::new(
                            operation,
                            world,
                            MoogleMailErrorCategory::ResponseTooLarge,
                            false,
                        )
                    });
                }
                body.0.extend_from_slice(&chunk);
            }
            Ok(std::mem::take(&mut body.0))
        })
        .await
        .map_err(|diagnostic_cause| {
            mail_signal(
                self.external_metrics.as_ref(),
                world,
                operation,
                auction_application::external_metrics::MailSignal::Timeout,
            );
            safe_diagnostics::warn(
                "mooglemail_read",
                "read_response_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(world.as_str()),
                    write_started: Some(write_started),
                    ..Default::default()
                },
            );
            read_error()
        })?
    }

    fn configuration(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::Configuration,
            false,
        )
    }

    fn configuration_for(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::new(operation, world, MoogleMailErrorCategory::Configuration, false)
    }

    /// 账号忙是临时限流分类：不表示 Cookie 失效，也不表示写入已经发生。
    fn account_busy_error(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::new(operation, world, MoogleMailErrorCategory::AccountBusy, true)
    }

    fn write_unknown(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::after_write(operation, world, MoogleMailErrorCategory::WriteUnknown)
    }

    fn protocol(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::ProtocolDrift,
            false,
        )
    }

    fn response_too_large(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::ResponseTooLarge,
            false,
        )
    }
}

/// 响应头仅保留状态码；未取得完整、合规大小的正文时明确保持 None。
#[derive(Default)]
struct DiagnosticResponse {
    status: Option<u16>,
    body: Option<Vec<u8>>,
}

impl Drop for DiagnosticResponse {
    fn drop(&mut self) {
        if let Some(body) = &mut self.body {
            body.fill(0);
        }
    }
}

pub(super) struct SensitiveBytes(pub(super) Vec<u8>);

impl Drop for SensitiveBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}
