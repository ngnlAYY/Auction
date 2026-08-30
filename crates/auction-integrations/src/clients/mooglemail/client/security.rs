//! MoogleMail HTTP 客户端的安全响应判定、限速器和敏感数据边界。

use super::*;

/// 构造不跟随重定向、关闭环境代理的 MoogleMail HTTP client。
pub(super) fn build_mooglemail_http_client() -> Result<Client, reqwest::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build()
}

pub(super) fn is_html_content_type(headers: &header::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| matches!(value.trim(), "text/html" | "application/xhtml+xml"))
}

pub(crate) fn is_hentaiverse_login_response(
    status: StatusCode,
    location: Option<&header::HeaderValue>,
    body: &[u8],
) -> bool {
    if status.is_redirection() {
        let location = location.and_then(|value| value.to_str().ok()).unwrap_or_default();
        let location = location.to_ascii_lowercase();
        if location.contains("bounce_login") {
            return true;
        }
    }
    let body = String::from_utf8_lossy(body).to_ascii_lowercase();
    body.contains("bounce_login.php")
        || body.contains("you are not logged in")
        || body.contains("this page requires you to log on")
        || body.contains("you must be logged on to visit the hentaiverse")
}

pub(super) fn is_session_expired(
    status: StatusCode,
    location: Option<&header::HeaderValue>,
    body: &[u8],
) -> bool {
    if is_hentaiverse_login_response(status, location, body) {
        return true;
    }
    status.is_redirection()
        && location
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().contains("e-hentai.org"))
}

pub(super) fn post_allowed(
    target_operation: MoogleMailOperation,
    form_operation: MoogleMailOperation,
) -> bool {
    match target_operation {
        MoogleMailOperation::Compose => matches!(
            form_operation,
            MoogleMailOperation::AttachAdd
                | MoogleMailOperation::AttachCod
                | MoogleMailOperation::Send
        ),
        MoogleMailOperation::Detail => {
            matches!(form_operation, MoogleMailOperation::ReturnMessage | MoogleMailOperation::Take)
        }
        _ => false,
    }
}

pub(super) struct IntervalGate {
    interval: Duration,
    next: Mutex<Instant>,
}

impl IntervalGate {
    pub(super) fn new(interval: Duration) -> Self {
        Self { interval, next: Mutex::new(Instant::now()) }
    }

    pub(super) async fn wait(&self) -> Result<(), ()> {
        let delay = {
            let mut next = self.next.lock().map_err(|_| ())?;
            let now = Instant::now();
            let reserved = (*next).max(now);
            *next = reserved.checked_add(self.interval).ok_or(())?;
            reserved.saturating_duration_since(now)
        };
        if !delay.is_zero() {
            time::sleep(delay).await;
        }
        Ok(())
    }
}
