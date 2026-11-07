//! 登录响应只保留短时自有缓冲；不记录 URL、headers、正文或任何远端错误细节。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use auction_application::admin::AdminCredentialError;
use auction_application::core::ApplicationError;
use reqwest::{Response, StatusCode, header};
use url::Url;

use super::{MAXIMUM_RESPONSE_BYTES, SecretBuffer, cookies::LoginCookies, form, unavailable};

pub(super) struct LoginPage {
    pub(super) body: SecretBuffer,
    pub(super) cookies: LoginCookies,
}

pub(super) async fn read(
    mut response: Response,
    endpoint: &Url,
    submitted: bool,
) -> Result<LoginPage, ApplicationError> {
    let status = response.status();
    if response.url().origin() != endpoint.origin()
        || status.is_redirection()
        || response.headers().contains_key(header::LOCATION)
        || response.content_length().is_some_and(|length| length > MAXIMUM_RESPONSE_BYTES as u64)
    {
        return Err(unavailable());
    }
    if response.headers().get("cf-mitigated").is_some_and(|value| value == "challenge") {
        return Err(AdminCredentialError::LoginChallenge.into());
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(AdminCredentialError::LoginBusy.into());
    }
    let html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| {
            value.trim().eq_ignore_ascii_case("text/html")
                || value.trim().eq_ignore_ascii_case("application/xhtml+xml")
        });
    if !html {
        return Err(unavailable());
    }
    for (name, value) in response.headers_mut().iter_mut() {
        if name == header::SET_COOKIE {
            value.set_sensitive(true);
        }
    }
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(MAXIMUM_RESPONSE_BYTES);
    let mut body = SecretBuffer(Vec::with_capacity(capacity));
    while let Some(chunk) = response.chunk().await.map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "credential_login",
            "read_response_chunk",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        unavailable()
    })? {
        if body.0.len().saturating_add(chunk.len()) > MAXIMUM_RESPONSE_BYTES {
            return Err(unavailable());
        }
        body.0.extend_from_slice(&chunk);
    }
    // GET 的成功正文由登录表单解析器检查，避免对同一页重复建立 DOM；POST 必须先排除挑战和明确拒绝。
    if submitted || status != StatusCode::OK {
        form::check_response(&body.0, status, submitted)?;
    }
    let cookies = LoginCookies::from_headers(response.headers(), endpoint)?;
    Ok(LoginPage { body, cookies })
}
