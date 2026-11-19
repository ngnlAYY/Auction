//! Admin 交付人工命令的处理器。

use std::net::SocketAddr;

use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryAcknowledgementCommand, DeliveryAdminAction, DeliveryAdminCommand,
    DeliveryDispositionCommandError,
};
use auction_contracts::{
    ADMIN_DELIVERY_RECONCILE_FAILURE_CODES, AdminDeliveryAcknowledgeRequest,
    AdminDeliveryAcknowledgeResponse, AdminDeliveryCommandRequest, AdminDeliveryCommandResponse,
    ApiResponse,
};
use auction_domain::DeliveryDispositionActorSource;
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use super::{feature_unavailable, map_delivery_error, map_snapshot};
use crate::errors::api_error;
use crate::middleware::boundary::request_id;
use crate::session::{authenticated_admin_mutation, no_store_headers};
use crate::state::AdminApiState;

/// release/retry 的旧 v1 请求体只有两个字段，不能因浏览器共用 DTO 而放宽未知键。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SimpleDeliveryCommandRequest {
    command_id: Uuid,
    expected_fencing_version: i64,
}

/// 强制释放仍处于 claimed 的 delivery。
pub(crate) async fn release(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute(state, address, delivery_id, headers, uri, body, DeliveryAdminAction::Release).await
}

/// 操作者确认无资金/资产影响的历史失败已经处置；不改变投递状态，只追加解除告警的处置事实。
pub(crate) async fn acknowledge(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "处置命令不接受查询参数")
            .into_response();
    }
    let Ok(input) = serde_json::from_slice::<AdminDeliveryAcknowledgeRequest>(&body) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
            .into_response();
    };
    let Ok(delivery_id) = Uuid::parse_str(&delivery_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_id", "交付 ID 无效")
            .into_response();
    };
    if delivery_id.is_nil()
        || input.command_id.is_nil()
        || input.expected_fencing_version <= 0
        || input.reason.is_empty()
        || input.reason.len() > 2048
    {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "处置命令参数无效")
            .into_response();
    }
    let Some(port) = state.delivery_disposition() else {
        return feature_unavailable();
    };
    let request_hash = DeliveryAcknowledgementCommand::request_hash(
        delivery_id,
        input.expected_fencing_version,
        &identity.actor,
        &input.reason,
    );
    let command = DeliveryAcknowledgementCommand {
        command_id: input.command_id,
        delivery_id,
        expected_fencing_version: input.expected_fencing_version,
        actor_source: DeliveryDispositionActorSource::Admin,
        actor_id: identity.actor,
        reason: input.reason,
        operator_identity: None,
        request_id: request_id(&headers),
        request_hash,
    };
    match port.acknowledge(command).await {
        Ok(result) => (
            StatusCode::OK,
            no_store_headers(),
            Json(ApiResponse::ok(
                "投递已确认处置",
                AdminDeliveryAcknowledgeResponse {
                    delivery_id: result.delivery_id.to_string(),
                    fencing_version: result.fencing_version,
                    kind: result.kind,
                },
            )),
        )
            .into_response(),
        Err(ApplicationError::DeliveryDisposition(error)) => disposition_error(error),
        Err(error) => {
            let kind = error.safe_kind();
            tracing::warn!(kind, "delivery disposition command failed");
            api_error(StatusCode::SERVICE_UNAVAILABLE, "delivery_unavailable", "交付处置暂不可用")
                .into_response()
        }
    }
}

fn disposition_error(error: DeliveryDispositionCommandError) -> Response {
    match error {
        DeliveryDispositionCommandError::Invalid => {
            api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "处置命令参数无效")
        }
        DeliveryDispositionCommandError::NotFound => {
            api_error(StatusCode::NOT_FOUND, "delivery_not_found", "交付不存在")
        }
        DeliveryDispositionCommandError::Stale => {
            api_error(StatusCode::CONFLICT, "stale_delivery_state", "交付已被其他命令推进")
        }
        DeliveryDispositionCommandError::AlreadyDisposed => {
            api_error(StatusCode::CONFLICT, "already_disposed", "该交付已有解除告警的处置")
        }
        DeliveryDispositionCommandError::NotAcknowledgeable => {
            api_error(StatusCode::UNPROCESSABLE_ENTITY, "not_acknowledgeable", "该交付不可确认处置")
        }
        DeliveryDispositionCommandError::CommandConflict => {
            api_error(StatusCode::CONFLICT, "command_conflict", "命令 ID 已用于不同请求")
        }
        DeliveryDispositionCommandError::Unavailable => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "delivery_unavailable", "交付处置暂不可用")
        }
    }
    .into_response()
}

/// 把明确失败的 delivery 重新放回 pending。
pub(crate) async fn retry(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute(state, address, delivery_id, headers, uri, body, DeliveryAdminAction::RetryFailed).await
}

/// 对结果未知的 delivery 执行人工决策。
pub(crate) async fn reconcile(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "对账命令不接受查询参数")
            .into_response();
    }
    let input = match serde_json::from_slice::<AdminDeliveryCommandRequest>(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let Some(action_name) = input.action.as_deref() else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "对账动作无效")
            .into_response();
    };
    let Some(action) = reconcile_action(action_name) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "对账动作无效")
            .into_response();
    };
    execute_command(state, headers, delivery_id, action, input, identity.actor).await
}

async fn execute(
    state: AdminApiState,
    address: SocketAddr,
    delivery_id: String,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
    action: DeliveryAdminAction,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "交付命令不接受查询参数")
            .into_response();
    }
    let input = match serde_json::from_slice::<SimpleDeliveryCommandRequest>(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let input = AdminDeliveryCommandRequest {
        command_id: input.command_id,
        expected_fencing_version: input.expected_fencing_version,
        action: None,
        external_reference: None,
        failure_code: None,
    };
    execute_command(state, headers, delivery_id, action, input, identity.actor).await
}

async fn execute_command(
    state: AdminApiState,
    headers: HeaderMap,
    delivery_id: String,
    action: DeliveryAdminAction,
    input: AdminDeliveryCommandRequest,
    actor_id: String,
) -> Response {
    let Ok(delivery_id) = Uuid::parse_str(&delivery_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_id", "交付 ID 无效")
            .into_response();
    };
    if delivery_id.is_nil() || input.command_id.is_nil() || input.expected_fencing_version <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "交付命令参数无效")
            .into_response();
    }
    let Some(port) = state.delivery_admin() else {
        return feature_unavailable();
    };
    let Some((external_reference, failure_code)) =
        normalize_action_fields(action, input.external_reference, input.failure_code)
    else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "交付命令参数无效")
            .into_response();
    };
    let request_hash = action.request_hash(
        delivery_id,
        input.expected_fencing_version,
        external_reference.as_deref(),
        failure_code.as_deref(),
        &actor_id,
    );
    let command = DeliveryAdminCommand {
        command_id: input.command_id,
        delivery_id,
        expected_fencing_version: input.expected_fencing_version,
        action,
        external_reference,
        failure_code,
        actor_id,
        request_id: request_id(&headers),
        request_hash,
    };
    let result = match port.execute(command).await {
        Ok(result) => result,
        Err(error) => return map_delivery_error(error),
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            delivery_action_message(action),
            AdminDeliveryCommandResponse { delivery: map_snapshot(result.snapshot) },
        )),
    )
        .into_response()
}

fn normalize_action_fields(
    action: DeliveryAdminAction,
    external_reference: Option<String>,
    failure_code: Option<String>,
) -> Option<(Option<String>, Option<String>)> {
    let external_reference = normalize_empty(external_reference);
    let failure_code = normalize_empty(failure_code);
    let valid = match action {
        DeliveryAdminAction::Release
        | DeliveryAdminAction::RetryFailed
        | DeliveryAdminAction::ReconcileRetry => {
            external_reference.is_none() && failure_code.is_none()
        }
        DeliveryAdminAction::ReconcileSucceeded => {
            is_trimmed_non_empty(external_reference.as_deref()) && failure_code.is_none()
        }
        // 新判失败只接受固定原因码，避免无语义自由文本进入投递与履约失败码。
        DeliveryAdminAction::ReconcileFailed => {
            external_reference.is_none()
                && failure_code
                    .as_deref()
                    .is_some_and(|code| ADMIN_DELIVERY_RECONCILE_FAILURE_CODES.contains(&code))
        }
    };

    if valid { Some((external_reference, failure_code)) } else { None }
}

fn normalize_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn is_trimmed_non_empty(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value == value.trim())
}

fn delivery_action_message(action: DeliveryAdminAction) -> &'static str {
    match action {
        DeliveryAdminAction::Release => "交付已释放",
        DeliveryAdminAction::RetryFailed => "失败交付已进入重试队列",
        DeliveryAdminAction::ReconcileRetry
        | DeliveryAdminAction::ReconcileSucceeded
        | DeliveryAdminAction::ReconcileFailed => "交付对账已完成",
    }
}

fn reconcile_action(value: &str) -> Option<DeliveryAdminAction> {
    match value {
        "retry" => Some(DeliveryAdminAction::ReconcileRetry),
        "succeeded" => Some(DeliveryAdminAction::ReconcileSucceeded),
        "failed" => Some(DeliveryAdminAction::ReconcileFailed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use auction_application::delivery::DeliveryDispositionCommandError;
    use axum::http::StatusCode;

    use super::disposition_error;
    use crate::server::contract_fixtures::assert_http;

    #[tokio::test]
    async fn delivery_disposition_error_contract_fixtures() {
        for (name, error) in [
            ("admin/error-already-disposed", DeliveryDispositionCommandError::AlreadyDisposed),
            ("admin/error-stale-delivery-disposition", DeliveryDispositionCommandError::Stale),
        ] {
            assert_http(name, StatusCode::CONFLICT, disposition_error(error)).await;
        }
    }
}

#[cfg(test)]
mod contract_tests {
    use std::sync::Arc;

    use auction_application::core::ApplicationFuture;
    use auction_application::delivery::{
        DeliveryAcknowledgementCommand, DeliveryAcknowledgementResult, DeliveryDispositionPort,
    };
    use auction_domain::{DeliveryAcknowledgementFacts, validate_acknowledgement};
    use axum::http::{Method, StatusCode};
    use serde_json::json;

    use crate::server::contract_fixtures::{admin_request, admin_state, assert_http};

    const DELIVERY_ID: uuid::Uuid = uuid::Uuid::from_u128(0x4000_8000_0000_0000_0023);
    const DELIVERY_FENCE: i64 = 2;

    struct Disposition;

    impl DeliveryDispositionPort for Disposition {
        fn acknowledge(
            &self,
            command: DeliveryAcknowledgementCommand,
        ) -> ApplicationFuture<'_, DeliveryAcknowledgementResult> {
            Box::pin(async move {
                command.validate().expect("合法处置命令");
                assert_eq!(command.delivery_id, DELIVERY_ID);
                assert_eq!(command.expected_fencing_version, DELIVERY_FENCE);
                // 确认只追加处置事实，既有失败状态和 fence 保持原值。
                validate_acknowledgement(DeliveryAcknowledgementFacts {
                    status: "failed",
                    origin_kind: "mail_intake",
                    credits: Some(0),
                    cod: Some(0),
                    equipment_count: 0,
                    has_fulfillment_obligation: false,
                })
                .expect("无资产责任的历史通知失败");
                Ok(DeliveryAcknowledgementResult {
                    command_id: command.command_id,
                    delivery_id: command.delivery_id,
                    fencing_version: DELIVERY_FENCE,
                    kind: "operator_acknowledged".to_owned(),
                    replayed: false,
                })
            })
        }
    }

    #[tokio::test]
    async fn delivery_acknowledgement_contract_fixtures() {
        let state = admin_state().with_delivery_disposition(Some(Arc::new(Disposition)));
        let path = "/api/v1/admin/deliveries/00000000-0000-4000-8000-000000000023/acknowledge";
        let body = json!({
            "command_id":"00000000-0000-4000-8000-000000000007",
            "expected_fencing_version":2, "reason":"已核对历史通知失败"
        });
        let response = admin_request(state, Method::POST, path, body.to_string().into()).await;
        assert_http("admin/delivery-acknowledge-command", StatusCode::OK, response).await;
    }
}
