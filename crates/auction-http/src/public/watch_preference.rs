//! 当前会话账号统一警告渠道；严格 JSON、CSRF 与无查询参数边界。
use std::net::SocketAddr;

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::public::{
    LotWatchError, LotWatchPreference, LotWatchPreferencePort, SetLotWatchPreferenceCommand,
};
use auction_contracts::ApiResponse;
use auction_contracts::{PublicLotWatchPreferenceResponse, PublicSetLotWatchPreferenceRequest};
use auction_domain::CommandId;
use axum::Json;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

use crate::diagnostics;
use crate::errors::{api_error, query_rejection};
use crate::middleware::strict_json;
use crate::query::parse_public_uuid;
use crate::session::{
    authenticated_public_identity, authenticated_public_mutation_identity, no_store_headers,
    session_guard,
};
use crate::state::PublicApiState;

pub(crate) async fn get_watch_preference(
    State(state): State<PublicApiState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|q| !q.is_empty()) {
        return query_rejection().into_response();
    }
    let Some(port) = state.watch_preference() else {
        return failure(LotWatchError::Unavailable);
    };
    let pm_available = load_pm_available(port, identity.user_id).await;
    match port.get(identity.user_id).await {
        Ok(value) if valid(&value, identity.user_id) => success(value, pm_available),
        Ok(_) => failure(LotWatchError::Unavailable),
        Err(error) => failure(error),
    }
}

pub(crate) async fn set_watch_preference(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    if let Some(response) = session_guard(&state, &parts.headers, address, true, false) {
        return response;
    }
    let identity = match authenticated_public_mutation_identity(&state, &parts.headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    if parts.uri.query().is_some_and(|q| !q.is_empty()) {
        return query_rejection().into_response();
    }
    let input: PublicSetLotWatchPreferenceRequest =
        match strict_json::decode_after_guard(&parts.headers, body, 4 << 10).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    let Ok(command_id) = parse_public_uuid(&input.command_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    };
    // 渠道偏好闭集 none/mm/pm/both；其余取值在进入用例前失败关闭。
    if auction_domain::LotWatchChannels::preference(&input.channel).is_none()
        || input.expected_version <= 0
    {
        return failure(LotWatchError::Invalid);
    }
    let Some(port) = state.watch_preference() else {
        return failure(LotWatchError::Unavailable);
    };
    match port
        .set(SetLotWatchPreferenceCommand {
            command_id: CommandId::from_uuid(command_id),
            user_id: identity.user_id,
            channel: input.channel.clone(),
            expected_version: input.expected_version,
        })
        .await
    {
        Ok(value)
            if valid(&value, identity.user_id)
                && value.channel == input.channel
                && Some(value.version) == input.expected_version.checked_add(1) =>
        {
            success(value, load_pm_available(port, identity.user_id).await)
        }
        Ok(_) => failure(LotWatchError::Unavailable),
        Err(error) => failure(error),
    }
}
/// PM 渠道可用性读取失败时失败关闭为不可用，但必须留下安全诊断，不能伪装成稳定的账号状态。
async fn load_pm_available(port: &dyn LotWatchPreferencePort, user_id: uuid::Uuid) -> bool {
    match port.pm_available(user_id).await {
        Ok(available) => available,
        Err(cause) => {
            diagnostics::warn(
                "public_watch_preference",
                "read_pm_available",
                DiagnosticError::diagnostic_kind(&cause),
                DiagnosticContext::default(),
            );
            false
        }
    }
}
fn valid(value: &LotWatchPreference, user_id: uuid::Uuid) -> bool {
    value.user_id == user_id
        && value.version > 0
        && auction_domain::LotWatchChannels::preference(&value.channel).is_some()
}
fn success(value: LotWatchPreference, pm_available: bool) -> Response {
    let data = PublicLotWatchPreferenceResponse {
        user_id: value.user_id.to_string(),
        channel: value.channel,
        version: value.version,
        pm_available,
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("竞价警告渠道已确认", data)))
        .into_response()
}
fn failure(error: LotWatchError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || {
        let (status, code, message) = match error {
            LotWatchError::Invalid => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_watch_preference", "警告渠道或版本无效")
            }
            LotWatchError::Conflict => {
                (StatusCode::CONFLICT, "watch_preference_conflict", "警告渠道命令或版本冲突")
            }
            LotWatchError::PmUnavailable => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "pm_channel_unavailable",
                "PM 提醒需要已启用的论坛同步与已绑定论坛账号",
            ),
            _ => (StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "警告渠道暂不可用"),
        };
        api_error(status, code, message).into_response()
    };
    let response = body();
    diagnostics::mapped_response(
        "lot_watch_preference",
        "respond_command_failure",
        diagnostic_kind,
        response,
    )
}

#[cfg(test)]
mod tests {
    use auction_application::public::LotWatchError;
    use axum::http::StatusCode;

    use super::failure;
    use crate::server::contract_fixtures::assert_http;

    #[tokio::test]
    async fn watch_preference_error_contract_fixtures() {
        for (name, status, error) in [
            (
                "public/error-invalid-watch-preference",
                StatusCode::UNPROCESSABLE_ENTITY,
                LotWatchError::Invalid,
            ),
            (
                "public/error-watch-preference-conflict",
                StatusCode::CONFLICT,
                LotWatchError::Conflict,
            ),
            (
                "public/error-pm-channel-unavailable",
                StatusCode::UNPROCESSABLE_ENTITY,
                LotWatchError::PmUnavailable,
            ),
        ] {
            assert_http(name, status, failure(error)).await;
        }
    }
}
