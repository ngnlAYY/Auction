//! Worker control 的 DTO、响应编码与请求头校验。
//!
//! 该文件只承载 Worker control 的单一职责，协议边界和安全约束保持由 transport 父模块统一装配。

use super::*;

/// Worker liveness 响应数据。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveResult {
    pub state: String,
}

/// Worker readiness 响应数据。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadyResult {
    pub ready: bool,
}

/// Worker 状态响应数据；不包含配置、凭据或错误正文。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatusResult {
    pub mail_pollers: usize,
    pub ready: bool,
    pub state: String,
    pub version: String,
}

/// Worker 手动轮询请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerPollCommand {
    pub command_id: String,
}

/// Worker 手动轮询响应。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerPollResult {
    pub accepted: bool,
    pub command_id: String,
    pub pollers: usize,
}

#[derive(Debug, Clone, Copy, Error)]
pub(super) enum TriggerError {
    #[error("invalid trigger command")]
    InvalidCommand,
    #[error("worker is not ready")]
    NotReady,
    #[error("worker has no MoogleMail poller")]
    NoPollers,
    #[error("worker control state unavailable")]
    Unavailable,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlEnvelope<T> {
    pub(super) code: String,
    pub(super) message: String,
    pub(super) data: Option<T>,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ResponseCode {
    Ok,
    Accepted,
    NotReady,
    Unavailable,
    Invalid,
}

impl ResponseCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Accepted => "accepted",
            Self::NotReady => "not_ready",
            Self::Unavailable => "unavailable",
            Self::Invalid => "invalid_command_id",
        }
    }
}

pub(super) fn response<T: Serialize>(
    status: StatusCode,
    code: ResponseCode,
    message: &'static str,
    data: Option<T>,
    request_id: &str,
) -> Response {
    let mut response = (
        status,
        [
            (HEADER_PROTOCOL_VERSION, WORKER_CONTROL_PROTOCOL_VERSION),
            (HEADER_REQUEST_ID, request_id),
        ],
        Json(ControlEnvelope { code: code.as_str().to_owned(), message: message.to_owned(), data }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

pub(super) fn request_id(headers: &HeaderMap) -> Result<String, ()> {
    let values = headers.get_all(HEADER_REQUEST_ID);
    let mut iter = values.iter();
    let Some(value) = iter.next() else {
        return Ok(Uuid::new_v4().to_string());
    };
    if iter.next().is_some() {
        return Err(());
    }
    let Ok(value) = value.to_str() else {
        return Err(());
    };
    if value.is_empty()
        || value.len() > 64
        || value != value.trim()
        || value.chars().any(char::is_control)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(());
    }
    Ok(value.to_owned())
}

pub(super) fn invalid_request_id_response() -> Response {
    response(
        StatusCode::BAD_REQUEST,
        ResponseCode::Invalid,
        "请求标识无效",
        Option::<()>::None,
        &Uuid::new_v4().to_string(),
    )
}
