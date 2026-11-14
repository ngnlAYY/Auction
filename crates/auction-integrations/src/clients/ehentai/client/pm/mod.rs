//! PM 准备、单次提交和同账号 Sent 对账；令牌与 Cookie 只驻留本次内存会话。

use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::forum::{
    ForumPmError, ForumPmFuture, ForumPmMessage, ForumPmReceipt, PreparedForumPm,
};
use reqwest::{StatusCode, header};
use url::Url;

use super::super::credentials::EhentaiCredentialSession;
use super::{EhentaiClient, request};
use crate::clients::ehentai::credentials::EhentaiCookie;
use crate::clients::ehentai::error::{EhentaiError, EhentaiErrorCategory, EhentaiOperation};
use crate::clients::ehentai::forms::{
    cookie_header, is_loopback_host, is_session_expired, same_origin, valid_topic_id,
};
use crate::clients::ehentai::{PRODUCTION_ENDPOINT, USER_AGENT};
use crate::clients::metrics::{
    ForumRequestGuard, forum_pm_category, forum_pm_request_result, forum_signal,
};
use crate::safe_diagnostics;

mod form;
mod protocol;
mod sent;

use form::PmForm;
use protocol::validate_message;

// 全次扫描预算独立于每响应上限；Worker 另以 item timeout 包住准备、发送及回读。
const MAXIMUM_SCAN_PAGES: usize = 8;
const MAXIMUM_SCAN_BYTES: usize = 8 * 1024 * 1024;
const MAXIMUM_CANDIDATES: usize = 8;

/// 不实现 Debug/序列化，避免表单令牌经通用诊断泄漏；消费后不能再次 POST。
struct PreparedPm {
    client: EhentaiClient,
    session: EhentaiCredentialSession,
    message: ForumPmMessage,
    sender: String,
    form: PmForm,
    metrics_world: auction_application::external_metrics::ForumWorld,
}

impl PreparedForumPm for PreparedPm {
    fn sender_forum_account_id(&self) -> &str {
        &self.sender
    }

    fn send(mut self: Box<Self>) -> ForumPmFuture<'static, ForumPmReceipt> {
        Box::pin(async move {
            // 调用方必须先持久化 sending fence；本对象不存在重试、换号或重新准备分支。
            let mut operation = crate::clients::metrics::ForumPmOperationGuard::start(
                self.client.external_metrics(),
                self.metrics_world,
                auction_application::external_metrics::ForumPmPhase::Send,
                false,
            );
            let result = async {
                self.client
                    .submit_pm(
                        &self.form,
                        self.session.cookie(),
                        self.metrics_world,
                        &mut operation,
                    )
                    .await?;
                self.client
                    .scan_sent(
                        &self.message,
                        &self.sender,
                        self.session.cookie(),
                        self.metrics_world,
                    )
                    .await
            }
            .await;
            if matches!(result, Err(ForumPmError::Authentication)) {
                // 写后失效同样停用当前 Cookie，但不能据此认定未发送或授权重发。
                let _ = self.session.invalidate_current().await;
            }
            let write_started = operation.write_started();
            let failure_category = result.as_ref().err().map(|error| forum_pm_category(*error));
            let result = result
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "forum_pm",
                        "send_unknown",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext {
                            write_started: Some(write_started),
                            ..Default::default()
                        },
                    );
                    ForumPmError::OutcomeUnknown
                })
                .and_then(|receipt| receipt.ok_or(ForumPmError::OutcomeUnknown));
            if result.is_ok() {
                operation.finish(
                    auction_application::external_metrics::ForumPmOutcome::Proved,
                    auction_application::external_metrics::ForumPmCategory::None,
                );
            } else {
                // 未越过 POST 的错误保留已知分类；一旦越过 POST，结果只能按未知处理。
                let (outcome, category) = if write_started {
                    (
                        auction_application::external_metrics::ForumPmOutcome::Unknown,
                        auction_application::external_metrics::ForumPmCategory::OutcomeUnknown,
                    )
                } else {
                    (
                        auction_application::external_metrics::ForumPmOutcome::Error,
                        failure_category.unwrap_or(
                            auction_application::external_metrics::ForumPmCategory::None,
                        ),
                    )
                };
                operation.finish(outcome, category);
                if write_started {
                    self.client.external_metrics().observe(
                        auction_application::external_metrics::ExternalMetricsEvent::ForumWriteUnknown {
                            world: self.metrics_world,
                            operation: auction_application::external_metrics::ForumOperation::PmSend,
                        },
                    );
                }
            }
            result
        })
    }
}

impl EhentaiClient {
    pub(in crate::clients::ehentai) async fn prepare_pm(
        &self,
        message: &ForumPmMessage,
        mut session: EhentaiCredentialSession,
        metrics_world: auction_application::external_metrics::ForumWorld,
    ) -> Result<Box<dyn PreparedForumPm>, ForumPmError> {
        self.validate_pm_client()?;
        validate_message(message)?;
        let target = self.pm_url(&[
            ("act", "Msg"),
            ("CODE", "04"),
            ("MID", &message.recipient_forum_account_id),
        ])?;
        let result = self
            .pm_get(
                target.clone(),
                session.cookie(),
                metrics_world,
                auction_application::external_metrics::ForumOperation::PmPrepare,
            )
            .await;
        if matches!(result, Err(ForumPmError::Authentication)) {
            let _ = session.invalidate_current().await;
        }
        let source = result?;
        let (sender, form) = form::parse(&source, &target, message)?;
        Ok(Box::new(PreparedPm {
            client: self.clone(),
            session,
            message: message.clone(),
            sender,
            form,
            metrics_world,
        }))
    }

    pub(in crate::clients::ehentai) async fn reconcile_pm(
        &self,
        message: &ForumPmMessage,
        sender: &str,
        mut session: EhentaiCredentialSession,
        metrics_world: auction_application::external_metrics::ForumWorld,
    ) -> Result<Option<ForumPmReceipt>, ForumPmError> {
        self.validate_pm_client()?;
        validate_message(message)?;
        if !valid_topic_id(sender) {
            return Err(ForumPmError::Protocol);
        }
        let result = self.scan_sent(message, sender, session.cookie(), metrics_world).await;
        if matches!(result, Err(ForumPmError::Authentication)) {
            let _ = session.invalidate_current().await;
        }
        result
    }

    pub(super) fn validate_pm_client(&self) -> Result<(), ForumPmError> {
        let production = Url::parse(PRODUCTION_ENDPOINT).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_pm",
                "validate_endpoint",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            ForumPmError::Protocol
        })?;
        let allowed = same_origin(&self.base_url, &production)
            || (self.base_url.scheme() == "http" && is_loopback_host(&self.base_url));
        if !self.valid_for()
            || !allowed
            || !self.base_url.username().is_empty()
            || self.base_url.password().is_some()
            || self.base_url.query().is_some()
            || self.base_url.fragment().is_some()
            || self.base_url.path() != "/"
        {
            return Err(ForumPmError::Protocol);
        }
        if self.pm_client.is_none() {
            return Err(ForumPmError::Unavailable);
        }
        Ok(())
    }

    fn pm_url(&self, pairs: &[(&str, &str)]) -> Result<Url, ForumPmError> {
        self.index_url(pairs, EhentaiOperation::CheckSession).map_err(map_error)
    }

    // 目标资料只读路径复用安全 GET 连接池与 gate/owner，不创建 PM 操作。
    pub(super) async fn pm_get(
        &self,
        target: Url,
        cookie: &EhentaiCookie,
        metrics_world: auction_application::external_metrics::ForumWorld,
        metrics_operation: auction_application::external_metrics::ForumOperation,
    ) -> Result<String, ForumPmError> {
        self.wait_rate(EhentaiOperation::CheckSession, metrics_world, metrics_operation)
            .await
            .map_err(map_error)?;
        self.check_ownership(cookie).await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_pm",
                "check_read_owner",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            ForumPmError::Unavailable
        })?;
        let request_metrics = ForumRequestGuard::start(
            std::sync::Arc::clone(&self.external_metrics),
            metrics_world,
            metrics_operation,
        );
        let result = async {
            let response = self
                .pm_client
                .as_ref()
                .ok_or(ForumPmError::Unavailable)?
                .get(target)
                .header(header::USER_AGENT, USER_AGENT)
                .header(
                    header::COOKIE,
                    cookie_header(cookie, EhentaiOperation::CheckSession).map_err(map_error)?,
                )
                .timeout(self.request_timeout)
                .send()
                .await
                .map_err(|diagnostic_cause| {
                    if diagnostic_cause.is_timeout() {
                        forum_signal(
                            self.external_metrics.as_ref(),
                            metrics_world,
                            metrics_operation,
                            auction_application::external_metrics::ForumSignal::Timeout,
                        );
                    }
                    safe_diagnostics::warn(
                        "forum_pm",
                        "read_page",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    ForumPmError::Unavailable
                })?;
            let status = response.status();
            let location = response.headers().get(header::LOCATION).cloned();
            let html = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.split(';').next().is_some_and(|v| v.trim().eq_ignore_ascii_case("text/html"))
                });
            let mut body = request::SensitiveResponseBody(
                self.read_response(
                    response,
                    EhentaiOperation::CheckSession,
                    metrics_world,
                    metrics_operation,
                    false,
                )
                .await
                .map_err(map_error)?,
            );
            if is_session_expired(status, location.as_ref(), &body.0) {
                return Err(ForumPmError::Authentication);
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                forum_signal(
                    self.external_metrics.as_ref(),
                    metrics_world,
                    metrics_operation,
                    auction_application::external_metrics::ForumSignal::RateLimited,
                );
                return Err(ForumPmError::RateLimited);
            }
            if !status.is_success() || location.is_some() || !html {
                return Err(ForumPmError::Protocol);
            }
            // 正文等待和读后 owner 复核都由清理守卫覆盖；失锁响应不能继续作为凭据证明。
            self.check_ownership(cookie).await.map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "forum_pm",
                    "check_read_receipt_owner",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                ForumPmError::Unavailable
            })?;
            String::from_utf8(std::mem::take(&mut body.0)).map_err(|error| {
                let diagnostic = safe_diagnostics::project(&error);
                let mut bytes = error.into_bytes();
                bytes.fill(0);
                safe_diagnostics::warn(
                    "forum_pm",
                    "decode_response",
                    diagnostic,
                    DiagnosticContext::default(),
                );
                ForumPmError::Protocol
            })
        }
        .await;
        request_metrics.finish(forum_pm_request_result(&result));
        result
    }

    async fn submit_pm(
        &self,
        form: &PmForm,
        cookie: &EhentaiCookie,
        metrics_world: auction_application::external_metrics::ForumWorld,
        operation: &mut crate::clients::metrics::ForumPmOperationGuard,
    ) -> Result<(), ForumPmError> {
        let metrics_operation = auction_application::external_metrics::ForumOperation::PmSend;
        self.wait_rate(EhentaiOperation::CheckSession, metrics_world, metrics_operation)
            .await
            .map_err(map_error)?;
        self.check_ownership(cookie).await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_pm",
                "check_send_owner",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            ForumPmError::Unavailable
        })?;
        // 复用已有 URL 编码器，不为 PM 额外开启 reqwest feature。
        let encoded = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form.values.iter().map(|(name, value)| (name.as_str(), value.as_str())))
            .finish();
        // 只使用封闭白名单字段，不跟踪、不保存草稿；一次 send 调用仅有这一次 POST。
        let request_metrics = ForumRequestGuard::start(
            std::sync::Arc::clone(&self.external_metrics),
            metrics_world,
            metrics_operation,
        );
        let result = async {
            let client = self.pm_client.as_ref().ok_or(ForumPmError::Unavailable)?;
            let cookie =
                cookie_header(cookie, EhentaiOperation::CheckSession).map_err(map_error)?;
            // POST 即将发出；此后的取消与错误都属于结果未知，之前的错误保留原分类。
            operation.mark_write_started();
            let response = client
                .post(form.action.clone())
                .header(header::USER_AGENT, USER_AGENT)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, &self.origin)
                .header(header::REFERER, form.referer.as_str())
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(encoded)
                .timeout(self.request_timeout)
                .send()
                .await
                .map_err(|diagnostic_cause| {
                    if diagnostic_cause.is_timeout() {
                        forum_signal(
                            self.external_metrics.as_ref(),
                            metrics_world,
                            metrics_operation,
                            auction_application::external_metrics::ForumSignal::Timeout,
                        );
                    }
                    safe_diagnostics::warn(
                        "forum_pm",
                        "submit_message",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext { write_started: Some(true), ..Default::default() },
                    );
                    ForumPmError::OutcomeUnknown
                })?;
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                forum_signal(
                    self.external_metrics.as_ref(),
                    metrics_world,
                    metrics_operation,
                    auction_application::external_metrics::ForumSignal::RateLimited,
                );
            }
            let location = response.headers().get(header::LOCATION).cloned();
            let body = self
                .read_response(
                    response,
                    EhentaiOperation::CheckSession,
                    metrics_world,
                    metrics_operation,
                    true,
                )
                .await
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "forum_pm",
                        "read_send_receipt",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext { write_started: Some(true), ..Default::default() },
                    );
                    ForumPmError::OutcomeUnknown
                })?;
            if is_session_expired(status, location.as_ref(), &body) {
                return Err(ForumPmError::Authentication);
            }
            if !status.is_success() || location.is_some() {
                return Err(ForumPmError::OutcomeUnknown);
            }
            // HTTP 200 只是允许开始只读核对，并非发送回执。
            Ok(())
        }
        .await;
        request_metrics.finish(forum_pm_request_result(&result));
        result
    }
}

fn map_error(error: EhentaiError) -> ForumPmError {
    safe_diagnostics::warn(
        "forum_pm",
        "project_transport_failure",
        safe_diagnostics::project(&error),
        DiagnosticContext::default(),
    );
    match error.category() {
        EhentaiErrorCategory::SessionExpired => ForumPmError::Authentication,
        EhentaiErrorCategory::RateLimited => ForumPmError::RateLimited,
        EhentaiErrorCategory::CredentialUnavailable | EhentaiErrorCategory::RemoteTemporary => {
            ForumPmError::Unavailable
        }
        _ => ForumPmError::Protocol,
    }
}
