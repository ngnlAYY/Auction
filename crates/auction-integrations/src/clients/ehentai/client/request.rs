//! E-Hentai HTTP 请求、响应边界和固定 origin 校验。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

impl EhentaiClient {
    pub(super) async fn get_page(
        &self,
        operation: EhentaiOperation,
        target: Url,
        referer: Option<&Url>,
        cookie: &EhentaiCookie,
    ) -> Result<(Vec<u8>, StatusCode), EhentaiError> {
        self.get_page_with_budget(operation, target, referer, cookie, None).await
    }

    pub(super) async fn get_page_with_budget(
        &self,
        operation: EhentaiOperation,
        target: Url,
        referer: Option<&Url>,
        cookie: &EhentaiCookie,
        budget: Option<&AtomicUsize>,
    ) -> Result<(Vec<u8>, StatusCode), EhentaiError> {
        self.wait_rate(operation).await?;
        let mut request = self
            .client
            .get(target)
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie_header(cookie, operation)?);
        if let Some(referer) = referer {
            request = request.header(header::REFERER, referer.as_str());
        }
        self.check_ownership(cookie).await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "check_read_owner",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::credential(operation)
        })?;
        if budget.is_some_and(|budget| budget.load(Ordering::Relaxed) == 0) {
            return Err(EhentaiError::response_too_large(operation));
        }
        let response =
            request.timeout(self.request_timeout).send().await.map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "forum_topic",
                    "fetch_page",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EhentaiError::remote_temporary(operation, false)
            })?;
        let status = response.status();
        let location = response.headers().get(header::LOCATION).cloned();
        let mut body = SensitiveResponseBody(
            self.read_response_with_budget(response, operation, false, budget).await?,
        );
        if is_session_expired(status, location.as_ref(), &body.0) {
            return Err(EhentaiError::session_expired(operation));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(EhentaiError::rate_limited(operation, false));
        }
        if status.is_server_error() {
            return Err(EhentaiError::remote_temporary(operation, false));
        }
        if status == StatusCode::FORBIDDEN {
            return Err(classify_forbidden(operation, &body.0));
        }
        if !status.is_success() {
            return Err(EhentaiError::protocol(operation, false));
        }
        if location.is_some() || !body_looks_like_html(&body.0) {
            return Err(EhentaiError::protocol(operation, false));
        }
        // 等待正文时 owner 可能已失效；只有完成复核的原 session 才能交付本次读取。
        self.check_ownership(cookie).await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "check_receipt_owner",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::credential(operation)
        })?;
        Ok((std::mem::take(&mut body.0), status))
    }

    pub(super) async fn read_response(
        &self,
        response: Response,
        operation: EhentaiOperation,
        write_started: bool,
    ) -> Result<Vec<u8>, EhentaiError> {
        self.read_response_with_budget(response, operation, write_started, None).await
    }

    async fn read_response_with_budget(
        &self,
        mut response: Response,
        operation: EhentaiOperation,
        write_started: bool,
        budget: Option<&AtomicUsize>,
    ) -> Result<Vec<u8>, EhentaiError> {
        if response.content_length().is_some_and(|length| {
            length > self.maximum_response_bytes as u64
                || budget.is_some_and(|budget| length > budget.load(Ordering::Relaxed) as u64)
        }) {
            return Err(if write_started {
                EhentaiError::write_unknown(operation)
            } else {
                EhentaiError::response_too_large(operation)
            });
        }
        // 只按已受上限约束的长度预留，减少正常分页响应的扩容复制；逐块门禁仍是最终边界。
        let capacity = response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default()
            .min(self.maximum_response_bytes);
        let mut body = SensitiveResponseBody(Vec::with_capacity(capacity));
        while let Some(chunk) = response.chunk().await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "read_response",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );

            if write_started {
                EhentaiError::write_unknown(operation)
            } else {
                EhentaiError::remote_temporary(operation, false)
            }
        })? {
            if body.0.len().saturating_add(chunk.len()) > self.maximum_response_bytes
                || budget.is_some_and(|budget| {
                    // MSRV 尚未稳定 try_update，而新工具链已弃用 fetch_update；用同一 CAS 语义扣减。
                    let mut remaining = budget.load(Ordering::Relaxed);
                    loop {
                        let Some(next) = remaining.checked_sub(chunk.len()) else {
                            return true;
                        };
                        match budget.compare_exchange_weak(
                            remaining,
                            next,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        ) {
                            Ok(_) => return false,
                            Err(current) => remaining = current,
                        }
                    }
                })
            {
                return Err(if write_started {
                    EhentaiError::write_unknown(operation)
                } else {
                    EhentaiError::response_too_large(operation)
                });
            }
            body.0.extend_from_slice(&chunk);
        }
        Ok(std::mem::take(&mut body.0))
    }

    pub(super) fn resolve_same_origin(
        &self,
        base: &Url,
        reference: &str,
        operation: EhentaiOperation,
    ) -> Result<Url, EhentaiError> {
        let resolved = base.join(reference).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "resolve_same_origin",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        if !same_origin(&resolved, &self.base_url)
            || !resolved.username().is_empty()
            || resolved.password().is_some()
        {
            return Err(EhentaiError::protocol(operation, false));
        }
        Ok(resolved)
    }

    pub(super) fn index_url(
        &self,
        pairs: &[(&str, &str)],
        operation: EhentaiOperation,
    ) -> Result<Url, EhentaiError> {
        if self.origin.is_empty() {
            return Err(EhentaiError::configuration(operation));
        }
        let mut target = self.base_url.clone();
        target.set_path("/index.php");
        target.set_fragment(None);
        let query = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs.iter().copied())
            .finish();
        target.set_query(Some(&query));
        Ok(target)
    }

    pub(super) fn board_for_world(&self, world: SettlementWorld) -> i32 {
        if self.test_board_mode {
            return TEST_BOARD_ID;
        }
        match world {
            SettlementWorld::Main => self.main_board_id,
            SettlementWorld::Isekai => self.isekai_board_id,
        }
    }

    pub(super) fn valid_for(&self) -> bool {
        !self.origin.is_empty()
            && self.base_url.host_str().is_some()
            && (self.base_url.scheme() == "https"
                || (self.base_url.scheme() == "http" && is_loopback_host(&self.base_url)))
            && self.maximum_response_bytes > 0
            && self.maximum_response_bytes <= HARD_MAXIMUM_RESPONSE_BYTES
            && self.maximum_topic_pages > 0
            && self.maximum_topic_pages <= HARD_MAXIMUM_TOPIC_PAGES
            && !self.minimum_request_interval.is_zero()
            && !self.request_timeout.is_zero()
    }

    pub(super) async fn wait_rate(&self, operation: EhentaiOperation) -> Result<(), EhentaiError> {
        // 等待期间持有异步锁；取消尚未获准的请求不能预占后续时间槽。
        let mut next = self.gate.lock().await;
        let wait = next.saturating_duration_since(Instant::now());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let Some(next_deadline) = Instant::now().checked_add(self.minimum_request_interval) else {
            // 限速截止时间不可表示时不能继续发起外部请求，交给 Worker 安全退避。
            return Err(EhentaiError::remote_temporary(operation, false));
        };
        *next = next_deadline;
        Ok(())
    }
}

// 超限、网络失败和取消都清理已收取的部分正文；完整响应才移交协议解析器。
pub(super) struct SensitiveResponseBody(pub(super) Vec<u8>);

impl Drop for SensitiveResponseBody {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

fn classify_forbidden(operation: EhentaiOperation, body: &[u8]) -> EhentaiError {
    // HTTP 错误页只匹配 ASCII 协议标记，不复制或转码整页敏感正文。
    let contains =
        |marker: &[u8]| body.windows(marker.len()).any(|value| value.eq_ignore_ascii_case(marker));
    if [b"cloudflare".as_slice(), b"cf-ray", b"attention required", b"just a moment"]
        .iter()
        .any(|marker| contains(marker))
    {
        EhentaiError::cloudflare(operation)
    } else if [b"topic is locked".as_slice(), b"topic is closed", b"cannot reply", b"cannot edit"]
        .iter()
        .any(|marker| contains(marker))
    {
        EhentaiError::topic_locked(operation)
    } else {
        EhentaiError::permission_denied(operation)
    }
}
