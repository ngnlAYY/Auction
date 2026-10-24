//! 装备来源 HTTP 读取的有界结果和敏感缓冲生命周期。

use super::*;

pub(super) struct EquipmentFetchFailure {
    pub(super) category: MailEquipmentDiagnosticCategory,
    pub(super) status: Option<u16>,
    pub(super) elapsed_millis: Option<u64>,
    pub(super) body: Option<Vec<u8>>,
}

impl Drop for EquipmentFetchFailure {
    fn drop(&mut self) {
        if let Some(body) = self.body.as_mut() {
            body.fill(0);
        }
    }
}

pub(super) struct EquipmentFetchSuccess {
    pub(super) body: Vec<u8>,
    pub(super) status: u16,
    pub(super) elapsed_millis: Option<u64>,
}

pub(super) struct SensitiveResponseBody(pub(super) Vec<u8>);

impl Drop for SensitiveResponseBody {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

pub(super) fn elapsed_millis(started: StdInstant) -> Option<u64> {
    Some(started.elapsed().as_millis().min(300_000) as u64)
}

pub(super) fn nonzero_status(status: &AtomicU16) -> Option<u16> {
    match status.load(Ordering::Relaxed) {
        0 => None,
        status => Some(status),
    }
}

pub(super) fn observed_failure(
    category: MailEquipmentDiagnosticCategory,
    status: Option<u16>,
    started: StdInstant,
    body: Option<Vec<u8>>,
) -> EquipmentFetchError {
    EquipmentFetchError::Observed(EquipmentFetchFailure {
        category,
        status,
        elapsed_millis: elapsed_millis(started),
        body,
    })
}

/// 执行一次不重定向的来源请求；完整正文才会从此模块交给解析层。
pub(super) async fn document_once(
    adapter: &EquipmentSourceAdapter,
    target: Url,
    cookie: Option<&MoogleMailCookie>,
    status_slot: Option<Arc<AtomicU16>>,
) -> Result<EquipmentFetchSuccess, EquipmentFetchError> {
    let started = StdInstant::now();
    let mut request =
        adapter.client.get(target).header(reqwest::header::USER_AGENT, DEFAULT_USER_AGENT);
    if let Some(cookie) = cookie {
        let mut header = reqwest::header::HeaderValue::from_str(cookie.expose()).map_err(|_| {
            observed_failure(
                MailEquipmentDiagnosticCategory::CredentialUnavailable,
                None,
                started,
                None,
            )
        })?;
        header.set_sensitive(true);
        request = request.header(reqwest::header::COOKIE, header);
    }
    // 装备读取使用独立 reqwest client，但共享同一个 MoogleMail 凭据 owner。
    if let Some(ownership) =
        adapter.credentials.as_ref().and_then(|credentials| credentials.ownership())
        && !timeout(std::time::Duration::from_secs(1), ownership.check())
            .await
            .is_ok_and(|result| result.is_ok())
    {
        return Err(observed_failure(
            MailEquipmentDiagnosticCategory::CredentialUnavailable,
            None,
            started,
            None,
        ));
    }
    let mut response = request.send().await.map_err(|_| {
        EquipmentFetchError::Observed(EquipmentFetchFailure {
            category: MailEquipmentDiagnosticCategory::Network,
            status: None,
            elapsed_millis: elapsed_millis(started),
            body: None,
        })
    })?;
    if let Some(slot) = status_slot.as_ref() {
        slot.store(response.status().as_u16(), Ordering::Relaxed);
    }
    let status = response.status();
    if response.url().scheme() != adapter.endpoint.scheme()
        || response.url().host_str() != adapter.endpoint.host_str()
        || response.url().port_or_known_default() != adapter.endpoint.port_or_known_default()
    {
        return Err(observed_failure(
            MailEquipmentDiagnosticCategory::Redirect,
            Some(status.as_u16()),
            started,
            None,
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > adapter.policy.maximum_response_bytes as u64)
    {
        return Err(observed_failure(
            MailEquipmentDiagnosticCategory::ResponseTooLarge,
            Some(status.as_u16()),
            started,
            None,
        ));
    }
    let encoding = response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .trim();
    if !encoding.is_empty() && !encoding.eq_ignore_ascii_case("identity") {
        return Err(observed_failure(
            MailEquipmentDiagnosticCategory::ContentEncoding,
            Some(status.as_u16()),
            started,
            None,
        ));
    }
    let location = response.headers().get(reqwest::header::LOCATION).cloned();
    let is_html = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| matches!(value.trim(), "text/html" | "application/xhtml+xml"));
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(adapter.policy.maximum_response_bytes);
    let mut body = SensitiveResponseBody(Vec::with_capacity(capacity));
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        observed_failure(
            MailEquipmentDiagnosticCategory::Network,
            Some(status.as_u16()),
            started,
            None,
        )
    })? {
        let next = body.0.len().checked_add(chunk.len()).ok_or_else(|| {
            observed_failure(
                MailEquipmentDiagnosticCategory::ResponseTooLarge,
                Some(status.as_u16()),
                started,
                None,
            )
        })?;
        if next > adapter.policy.maximum_response_bytes {
            return Err(observed_failure(
                MailEquipmentDiagnosticCategory::ResponseTooLarge,
                Some(status.as_u16()),
                started,
                None,
            ));
        }
        body.0.extend_from_slice(&chunk);
    }
    if is_hentaiverse_login_response(status, location.as_ref(), &body.0) {
        return Err(EquipmentFetchError::Observed(EquipmentFetchFailure {
            category: MailEquipmentDiagnosticCategory::SessionExpired,
            status: Some(status.as_u16()),
            elapsed_millis: elapsed_millis(started),
            body: Some(std::mem::take(&mut body.0)),
        }));
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        let category = if status == StatusCode::TOO_MANY_REQUESTS {
            MailEquipmentDiagnosticCategory::RateLimited
        } else {
            MailEquipmentDiagnosticCategory::ServerError
        };
        return Err(EquipmentFetchError::Observed(EquipmentFetchFailure {
            category,
            status: Some(status.as_u16()),
            elapsed_millis: elapsed_millis(started),
            body: Some(std::mem::take(&mut body.0)),
        }));
    }
    if !status.is_success() {
        return Err(EquipmentFetchError::Observed(EquipmentFetchFailure {
            category: MailEquipmentDiagnosticCategory::HttpStatus,
            status: Some(status.as_u16()),
            elapsed_millis: elapsed_millis(started),
            body: Some(std::mem::take(&mut body.0)),
        }));
    }
    if !is_html {
        return Err(EquipmentFetchError::Observed(EquipmentFetchFailure {
            category: MailEquipmentDiagnosticCategory::ContentType,
            status: Some(status.as_u16()),
            elapsed_millis: elapsed_millis(started),
            body: Some(std::mem::take(&mut body.0)),
        }));
    }
    Ok(EquipmentFetchSuccess {
        body: std::mem::take(&mut body.0),
        status: status.as_u16(),
        elapsed_millis: elapsed_millis(started),
    })
}
