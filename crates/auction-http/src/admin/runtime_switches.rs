//! Admin 运行时开关接口：读取数据库意图与本进程实际快照；写入只开放外部访问暂停，
//! 其余开关仍只能经 CLI 修改。

use std::{net::SocketAddr, time::Duration};

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError, SafeErrorKind};
use auction_application::runtime_switches::{
    RuntimeSwitchCommand, RuntimeSwitchError, RuntimeSwitchEvent, RuntimeSwitchName,
    RuntimeSwitchOperation,
};
use auction_contracts::{
    AdminExternalAccessPauseRequest, AdminExternalAccessPauseResponse, ApiResponse,
    RuntimeSwitchAppliedDto, RuntimeSwitchIntentDto, RuntimeSwitchListResponse,
};
use axum::{
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::AdminApiState;
use crate::diagnostics;

/// Admin 暂停的最长期限；忘记恢复时外部访问最迟在此之后自动回落。
const MAX_ADMIN_PAUSE: chrono::TimeDelta = chrono::TimeDelta::days(7);
/// 数据库写入等待上限；超时后提交结果未知，只允许原样重试。
const PAUSE_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn get(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let Some((store, switches)) = state.runtime_switches() else {
        return (
            super::no_store_headers(),
            super::api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "feature_unavailable",
                "运行时开关不可用",
            ),
        )
            .into_response();
    };
    let read = match tokio::time::timeout(Duration::from_secs(1), store.read()).await {
        Ok(Ok(read)) => read,
        other => {
            let (phase, safe_kind) = match &other {
                Ok(Err(cause)) => ("read_switches", cause.diagnostic_kind()),
                Err(cause) => ("read_switches_timeout", diagnostics::timeout_kind(cause)),
                Ok(Ok(_)) => ("validate_switch_read", SafeErrorKind::DEPENDENCY_MISMATCH),
            };
            diagnostics::warn(
                "admin_runtime_switches",
                phase,
                safe_kind,
                DiagnosticContext::default(),
            );
            drop(other);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                super::no_store_headers(),
                super::api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "运行时开关读取失败",
                ),
            )
                .into_response();
        }
    };
    let status = switches.status();
    let database_intent = RuntimeSwitchName::ALL
        .into_iter()
        .map(|name| {
            let event = read.events.iter().find(|event| event.name == name);
            RuntimeSwitchIntentDto {
                name: name.label().to_owned(),
                revision: event.map_or(0, |event| event.revision),
                operation: event.map(|event| event.operation.label().to_owned()),
                value: event.and_then(|event| event.value.clone()),
                expires_at: event.and_then(|event| event.expires_at),
                actor_id: event.map(|event| event.actor_id.clone()),
                entrypoint: event.map(|event| event.entrypoint.clone()),
                reason: event.map(|event| event.reason.clone()),
            }
        })
        .collect();
    let admin_applied = status
        .switches
        .into_iter()
        .filter(|view| view.applies)
        .map(|view| RuntimeSwitchAppliedDto {
            name: view.name.label().to_owned(),
            revision: view.applied_revision,
            toml_baseline: view.toml_baseline,
            effective_value: view.effective_value,
            overridden: view.overridden,
        })
        .collect();
    (
        StatusCode::OK,
        super::no_store_headers(),
        axum::Json(ApiResponse::ok(
            "运行时开关已读取",
            RuntimeSwitchListResponse {
                database_time: read.database_time,
                database_intent,
                admin_last_success_age_seconds: status.last_success_age_seconds,
                admin_applied,
            },
        )),
    )
        .into_response()
}

/// Admin 暂停或恢复全部外部访问。只接受 `external_access_paused`，暂停必须带期限
/// （不超过 7 天）；命令经同一开关账本写入，同 ID 同参重放返回原事实，结果未知只能原样重试。
pub(crate) async fn pause_external_access(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let identity = match super::authenticated_admin_mutation(&state, &headers, address).await {
        Ok((_, identity)) => identity,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return invalid_pause("查询参数无效");
    }
    let Ok(request) = serde_json::from_slice::<AdminExternalAccessPauseRequest>(&body) else {
        return invalid_pause("请求 JSON 无效");
    };
    let command = match pause_command(request, identity.actor, Utc::now()) {
        Ok(command) => command,
        Err(message) => return invalid_pause(message),
    };
    let Some((store, _)) = state.runtime_switches() else {
        return pause_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "feature_unavailable",
            "运行时开关不可用",
        );
    };
    match tokio::time::timeout(PAUSE_WRITE_TIMEOUT, store.write(command)).await {
        Ok(Ok(event)) => (
            StatusCode::OK,
            super::no_store_headers(),
            axum::Json(ApiResponse::ok(
                "数据库已接受；各角色约 5 秒内刷新",
                AdminExternalAccessPauseResponse { accepted: intent_dto(&event) },
            )),
        )
            .into_response(),
        Ok(Err(error)) => {
            diagnostics::warn(
                "admin_runtime_switches",
                "write_external_access_pause",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            match error {
                RuntimeSwitchError::Invalid | RuntimeSwitchError::UnsafeRelaxation => {
                    invalid_pause("暂停命令无效或期限已过")
                }
                RuntimeSwitchError::Conflict => pause_error(
                    StatusCode::CONFLICT,
                    "runtime_switch_conflict",
                    "版本已变化或命令 ID 参数不一致；刷新后用新命令重试",
                ),
                RuntimeSwitchError::OutcomeUnknown => outcome_unknown(),
                RuntimeSwitchError::Corrupt | RuntimeSwitchError::Unavailable => pause_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "运行时开关存储不可用",
                ),
            }
        }
        Err(cause) => {
            diagnostics::warn(
                "admin_runtime_switches",
                "write_external_access_pause_timeout",
                diagnostics::timeout_kind(&cause),
                DiagnosticContext::default(),
            );
            outcome_unknown()
        }
    }
}

/// 把请求映射为开关命令；期限只校验上界，过去的期限由数据库锁内按新命令拒绝，
/// 已接受命令的原样重放不受当前时刻影响。
fn pause_command(
    request: AdminExternalAccessPauseRequest,
    actor_id: String,
    now: DateTime<Utc>,
) -> Result<RuntimeSwitchCommand, &'static str> {
    let command_id = Uuid::parse_str(&request.command_id)
        .ok()
        .filter(|id| !id.is_nil())
        .ok_or("命令 ID 无效")?;
    if request.expected_revision < 0 {
        return Err("期望版本无效");
    }
    let (operation, value, expires_at) = if request.paused {
        let until = request.expires_at.ok_or("暂停必须带到期时间")?;
        if until > now + MAX_ADMIN_PAUSE {
            return Err("暂停期限不能超过 7 天");
        }
        (RuntimeSwitchOperation::Set, Some(serde_json::json!(true)), Some(until))
    } else {
        if request.expires_at.is_some() {
            return Err("恢复不接受到期时间");
        }
        (RuntimeSwitchOperation::Clear, None, None)
    };
    let command = RuntimeSwitchCommand {
        command_id,
        name: RuntimeSwitchName::ExternalAccessPaused,
        expected_revision: request.expected_revision,
        operation,
        value,
        expires_at,
        expired_command_id: None,
        actor_id,
        operator_identity: None,
        entrypoint: "admin".to_owned(),
        reason: request.reason,
    };
    if command.validate().is_err() {
        return Err("暂停命令无效");
    }
    Ok(command)
}

fn intent_dto(event: &RuntimeSwitchEvent) -> RuntimeSwitchIntentDto {
    RuntimeSwitchIntentDto {
        name: event.name.label().to_owned(),
        revision: event.revision,
        operation: Some(event.operation.label().to_owned()),
        value: event.value.clone(),
        expires_at: event.expires_at,
        actor_id: Some(event.actor_id.clone()),
        entrypoint: Some(event.entrypoint.clone()),
        reason: Some(event.reason.clone()),
    }
}

fn invalid_pause(message: &'static str) -> Response {
    pause_error(StatusCode::UNPROCESSABLE_ENTITY, "invalid_runtime_switch", message)
}

fn outcome_unknown() -> Response {
    pause_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "outcome_unknown",
        "提交结果未知；请用原命令 ID 与原参数重试",
    )
}

fn pause_error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    (status, super::no_store_headers(), super::api_error(status, code, message)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(paused: bool, expires_at: Option<DateTime<Utc>>) -> AdminExternalAccessPauseRequest {
        AdminExternalAccessPauseRequest {
            command_id: Uuid::new_v4().to_string(),
            expected_revision: 0,
            paused,
            expires_at,
            reason: "manual account use".to_owned(),
        }
    }

    fn at_micros(value: DateTime<Utc>) -> DateTime<Utc> {
        DateTime::from_timestamp_micros(value.timestamp_micros()).expect("微秒精度时间")
    }

    #[test]
    fn pause_maps_to_admin_set_with_absolute_expiry() {
        let now = Utc::now();
        let until = at_micros(now + chrono::TimeDelta::hours(2));
        let command = pause_command(request(true, Some(until)), "admin".to_owned(), now)
            .expect("合法暂停命令");
        assert_eq!(command.name, RuntimeSwitchName::ExternalAccessPaused);
        assert_eq!(command.operation, RuntimeSwitchOperation::Set);
        assert_eq!(command.value, Some(serde_json::json!(true)));
        assert_eq!(command.expires_at, Some(until));
        assert_eq!(command.entrypoint, "admin");
    }

    #[test]
    fn resume_maps_to_clear_without_expiry() {
        let command = pause_command(request(false, None), "admin".to_owned(), Utc::now())
            .expect("合法恢复命令");
        assert_eq!(command.operation, RuntimeSwitchOperation::Clear);
        assert_eq!(command.value, None);
        assert_eq!(command.expires_at, None);
    }

    #[test]
    fn invalid_pause_requests_fail_closed() {
        let now = Utc::now();
        assert!(pause_command(request(true, None), "admin".to_owned(), now).is_err());
        let too_long = at_micros(now + chrono::TimeDelta::days(8));
        assert!(pause_command(request(true, Some(too_long)), "admin".to_owned(), now).is_err());
        let soon = at_micros(now + chrono::TimeDelta::hours(1));
        assert!(pause_command(request(false, Some(soon)), "admin".to_owned(), now).is_err());
        let mut nil = request(true, Some(soon));
        nil.command_id = Uuid::nil().to_string();
        assert!(pause_command(nil, "admin".to_owned(), now).is_err());
        let mut negative = request(true, Some(soon));
        negative.expected_revision = -1;
        assert!(pause_command(negative, "admin".to_owned(), now).is_err());
        let mut blank = request(true, Some(soon));
        blank.reason = String::new();
        assert!(pause_command(blank, "admin".to_owned(), now).is_err());
    }
}
