//! E-Hentai HTTP 请求、响应边界和固定 origin 校验。

use super::*;

impl EhentaiClient {
    pub(super) async fn get_page(
        &self,
        operation: EhentaiOperation,
        target: Url,
        referer: Option<&Url>,
        cookie: &EhentaiCookie,
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
        self.check_ownership().await.map_err(|_| EhentaiError::credential(operation))?;
        let response = request
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|_| EhentaiError::remote_temporary(operation, false))?;
        let status = response.status();
        let location = response.headers().get(header::LOCATION).cloned();
        let body = self.read_response(response, operation, false).await?;
        if is_session_expired(status, location.as_ref(), &body) {
            return Err(EhentaiError::session_expired(operation));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(EhentaiError::rate_limited(operation, false));
        }
        if status.is_server_error() {
            return Err(EhentaiError::remote_temporary(operation, false));
        }
        if status == StatusCode::FORBIDDEN {
            return Err(classify_forbidden(operation, &body));
        }
        if !status.is_success() {
            return Err(EhentaiError::protocol(operation, false));
        }
        if location.is_some() || !body_looks_like_html(&body) {
            return Err(EhentaiError::protocol(operation, false));
        }
        Ok((body, status))
    }

    pub(super) async fn read_response(
        &self,
        mut response: Response,
        operation: EhentaiOperation,
        write_started: bool,
    ) -> Result<Vec<u8>, EhentaiError> {
        if response
            .content_length()
            .is_some_and(|length| length > self.maximum_response_bytes as u64)
        {
            return Err(if write_started {
                EhentaiError::write_unknown(operation)
            } else {
                EhentaiError::response_too_large(operation)
            });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            if write_started {
                EhentaiError::write_unknown(operation)
            } else {
                EhentaiError::remote_temporary(operation, false)
            }
        })? {
            if body.len().saturating_add(chunk.len()) > self.maximum_response_bytes {
                return Err(if write_started {
                    EhentaiError::write_unknown(operation)
                } else {
                    EhentaiError::response_too_large(operation)
                });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    pub(super) fn resolve_same_origin(
        &self,
        base: &Url,
        reference: &str,
        operation: EhentaiOperation,
    ) -> Result<Url, EhentaiError> {
        let resolved =
            base.join(reference).map_err(|_| EhentaiError::protocol(operation, false))?;
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

    pub(super) fn board_for_world(
        &self,
        world: SettlementWorld,
        _operation: EhentaiOperation,
    ) -> Result<i32, EhentaiError> {
        if self.test_board_mode {
            return Ok(TEST_BOARD_ID);
        }
        Ok(match world {
            SettlementWorld::Main => self.main_board_id,
            SettlementWorld::Isekai => self.isekai_board_id,
        })
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

fn classify_forbidden(operation: EhentaiOperation, body: &[u8]) -> EhentaiError {
    let evidence = String::from_utf8_lossy(body).to_ascii_lowercase();
    if ["cloudflare", "cf-ray", "attention required", "just a moment"]
        .iter()
        .any(|marker| evidence.contains(marker))
    {
        EhentaiError::cloudflare(operation)
    } else if ["topic is locked", "topic is closed", "cannot reply", "cannot edit"]
        .iter()
        .any(|marker| evidence.contains(marker))
    {
        EhentaiError::topic_locked(operation)
    } else {
        EhentaiError::permission_denied(operation)
    }
}
