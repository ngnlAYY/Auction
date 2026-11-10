//! Worker control 的 DTO、响应编码与请求头校验。

use super::*;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveResult {
    pub state: String,
}

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

/// 独立只读路径的进度响应，避免改变旧 status 的闭集。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MailProgressResult {
    pub(super) observed_at: DateTime<Utc>,
    pub(super) pollers: Vec<MailPollProgressSnapshot>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerPollCommand {
    pub command_id: String,
}

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
    /// 主动暂停；请求未发往远端。
    Paused,
    /// 恢复核对要求先暂停外部访问；请求未发往远端。
    NotPaused,
    /// 当前 Worker 未装配该远端读取能力。
    CapabilityUnavailable,
}

impl ResponseCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Accepted => "accepted",
            Self::NotReady => "not_ready",
            Self::Unavailable => "unavailable",
            Self::Invalid => "invalid_command_id",
            Self::Paused => "paused",
            Self::NotPaused => "not_paused",
            Self::CapabilityUnavailable => "capability_unavailable",
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
    if !auction_domain::valid_text(value, 64)
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

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::ControlEnvelope;
    use auction_application::external_accounts::{
        ExternalAccountStatus, ExternalForumAccountStatus, ExternalMailAccountStatus,
        ExternalMaintenanceState, ExternalMaintenanceStatus,
    };

    fn idle_status() -> ExternalAccountStatus {
        let observed_at = Utc::now();
        ExternalAccountStatus {
            observed_at,
            mail: ExternalMailAccountStatus {
                busy: false,
                busy_age_seconds: None,
                gate_wait_count: 0,
                gate_wait_seconds_total: 0,
                request_count: 0,
                last_request_at: None,
                last_success_at: None,
                last_failure_at: None,
            },
            forum: ExternalForumAccountStatus {
                last_request_at: None,
                last_success_at: None,
                last_failure_at: None,
                write_unknown_count: 0,
            },
            maintenance: ExternalMaintenanceStatus {
                state: ExternalMaintenanceState::Idle,
                last_started_at: None,
                last_success_at: None,
                last_failure_at: None,
                last_canceled_at: None,
                running_age_seconds: None,
            },
        }
    }

    #[test]
    fn external_accounts_envelope_round_trips() {
        let status = idle_status();
        let envelope = ControlEnvelope {
            code: "ok".to_owned(),
            message: "外部账号运行观测可用".to_owned(),
            data: Some(status.clone()),
        };
        let encoded = serde_json::to_value(&envelope).expect("encode control envelope");
        let decoded: ControlEnvelope<ExternalAccountStatus> =
            serde_json::from_value(encoded).expect("decode control envelope");
        assert_eq!(decoded.data, Some(status));
    }

    #[test]
    fn external_accounts_reject_unknown_fields() {
        let mut value = serde_json::to_value(idle_status()).expect("encode status");
        value["credential"] = serde_json::json!("secret");
        assert!(serde_json::from_value::<ExternalAccountStatus>(value).is_err());
    }

    #[test]
    fn external_accounts_reject_busy_without_duration() {
        let mut status = idle_status();
        status.mail.busy = true;
        status.mail.busy_age_seconds = None;
        assert!(!status.validate());
    }
}
