//! Worker control 的 DTO、响应编码与请求头校验。

use auction_application::operations::{
    WORKER_CONTROL_HEADER_REQUEST_ID as HEADER_REQUEST_ID,
    WORKER_CONTROL_HEADER_VERSION as HEADER_PROTOCOL_VERSION, WORKER_CONTROL_PROTOCOL_VERSION,
};
use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

// 线上 DTO 与响应外壳的唯一 owner 在 application；服务端沿用短名。
pub use auction_application::operations::{
    WorkerControlEnvelope as ControlEnvelope, WorkerControlLiveResult as LiveResult,
    WorkerControlReadyResult as ReadyResult, WorkerControlStatusResult as StatusResult,
    WorkerControlTriggerPollCommand as TriggerPollCommand,
    WorkerControlTriggerPollResult as TriggerPollResult,
};

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
    use auction_application::external_accounts::{
        ExternalAccountStatus, ExternalForumAccountStatus, ExternalMailAccountStatus,
        ExternalMaintenanceState, ExternalMaintenanceStatus,
    };
    use chrono::Utc;

    use super::ControlEnvelope;

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
