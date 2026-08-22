//! Admin 邮件轮询与入站审查 HTTP 适配器。
//!
//! 这里只解析查询、路径、CSRF 和稳定 DTO。邮件内容、决定元数据闭集、轮询版本 fence 和
//! 幂等命令由应用/存储 owner 负责；远程 MoogleMail trigger 仍由独立 Worker control 协议拥有。

use std::net::SocketAddr;

use auction_application::{
    AdminMailError, AdminMailIntakeDetail, AdminMailIntakeFilter, AdminMailIntakeMetadata,
    AdminMailIntakeSummary, AdminMailPollState, AdminMailPollStateRequest,
    AdminMailPollTriggerCommand, AdminMailPollingCommand, AdminMailTransactionError,
    ApplicationError, RequestHash, admin_mail_aggregate_id,
};
use auction_contracts::{
    AdminMailIntakeDetailResponse, AdminMailIntakeListResponse, AdminMailIntakeMetadataResponse,
    AdminMailIntakeSummaryResponse, AdminMailPaginationResponse, AdminMailPollTriggerRequest,
    AdminMailPollingRequest, AdminMailPollingResponse, AdminMailStateResponse,
    ApiErrorResponseWithData, ApiResponse,
};
use auction_domain::CommandId;
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::AdminApiState;

/// 读取指定邮箱的轮询状态。
pub(crate) async fn state(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query = match super::parse_query(&uri, &["mailbox_key"]) {
        Ok(query) if query.get("mailbox_key").is_some_and(|value| !value.is_empty()) => query,
        _ => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let request = AdminMailPollStateRequest {
        mailbox_key: query.get("mailbox_key").cloned().unwrap_or_default(),
    };
    if let Err(error) = request.validate() {
        return map_error(ApplicationError::from(error));
    }
    let Some(port) = state.admin_mail() else {
        return feature_unavailable();
    };
    let result = match port.poll_state(request).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate() {
        return super::internal_error();
    }
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "邮件轮询状态已加载",
            AdminMailStateResponse { polling: map_poll_state(result) },
        )),
    )
        .into_response()
}

/// 读取 Admin 入站邮件安全摘要列表。
pub(crate) async fn list_intakes(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query = match super::parse_query(
        &uri,
        &[
            "mailbox_key",
            "world_id",
            "kind",
            "status",
            "decision",
            "from",
            "to",
            "page",
            "page_size",
        ],
    ) {
        Ok(query) if query.values().all(|value| !value.is_empty()) => query,
        _ => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page = match super::parse_positive_i64(query.get("page"), 1, None) {
        Ok(page) => page,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page_size = match super::parse_positive_i64(
        query.get("page_size"),
        20,
        Some(auction_application::MAX_ADMIN_MAIL_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let received_from = match parse_time(query.get("from")) {
        Ok(value) => value,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let received_to = match parse_time(query.get("to")) {
        Ok(value) => value,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let request = AdminMailIntakeFilter {
        mailbox_key: query.get("mailbox_key").cloned(),
        world_id: query.get("world_id").cloned(),
        kind: query.get("kind").cloned(),
        status: query.get("status").cloned(),
        decision: query.get("decision").cloned(),
        received_from,
        received_to,
        page,
        page_size,
    };
    if let Err(error) = request.validate() {
        return map_error(ApplicationError::from(error));
    }
    let Some(port) = state.admin_mail() else {
        return feature_unavailable();
    };
    let result = match port.list_intakes(request.clone()).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate(&request) {
        return super::internal_error();
    }
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "邮件入站列表已加载",
            AdminMailIntakeListResponse {
                intakes: result.items.into_iter().map(map_summary).collect(),
                pagination: AdminMailPaginationResponse {
                    page: result.page,
                    page_size: result.page_size,
                    total: result.total,
                },
            },
        )),
    )
        .into_response()
}

/// 读取单条 Admin 入站邮件详情。
pub(crate) async fn detail_intake(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(intake_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
            .into_response();
    }
    let Ok(intake_id) = Uuid::parse_str(&intake_id) else {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_intake_id", "邮件入站 ID 无效")
            .into_response();
    };
    if intake_id.is_nil() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_intake_id", "邮件入站 ID 无效")
            .into_response();
    }
    let Some(port) = state.admin_mail() else {
        return feature_unavailable();
    };
    let result = match port.intake_detail(intake_id).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    let Some(result) = result else {
        return super::not_found_error().into_response();
    };
    if !result.summary.validate() || !result.metadata.validate() {
        return super::internal_error();
    }
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok("邮件入站详情已加载", map_detail(result))),
    )
        .into_response()
}

/// 以版本 fence 暂停或恢复指定邮箱轮询。
pub(crate) async fn set_paused(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(raw_csrf) = super::read_admin_cookie(&headers, super::ADMIN_CSRF_COOKIE) else {
        return super::authentication_error();
    };
    if !super::valid_admin_csrf_header(&headers, &raw_csrf) {
        return super::api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败")
            .into_response();
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "邮件轮询命令不接受查询参数",
        )
        .into_response();
    }
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminMailPollingRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    let aggregate_id = admin_mail_aggregate_id(&input.mailbox_key);
    let request_hash =
        match RequestHash::from_request("admin_mail_set_polling_paused", aggregate_id, &input) {
            Ok(hash) => hash,
            Err(_) => return super::internal_error(),
        };
    let Some(port) = state.admin_mail() else {
        return feature_unavailable();
    };
    let result = match port
        .set_polling_paused(AdminMailPollingCommand {
            command_id: CommandId::from_uuid(input.command_id),
            mailbox_key: input.mailbox_key.clone(),
            paused: input.paused,
            expected_state_version: input.expected_state_version,
            actor_id: identity.actor,
            request_id: None,
            request_hash,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate()
        || result.mailbox_key != input.mailbox_key
        || result.paused != input.paused
        || result.state_version != input.expected_state_version.saturating_add(1)
    {
        return super::internal_error();
    }
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "邮件轮询状态已更新",
            AdminMailStateResponse { polling: map_poll_state(result) },
        )),
    )
        .into_response()
}

/// 以版本 fence 和幂等命令立即唤醒指定邮箱的 Worker 轮询。
pub(crate) async fn trigger_poll(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(raw_csrf) = super::read_admin_cookie(&headers, super::ADMIN_CSRF_COOKIE) else {
        return super::authentication_error();
    };
    if !super::valid_admin_csrf_header(&headers, &raw_csrf) {
        return super::api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败")
            .into_response();
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "邮件轮询触发不接受查询参数",
        )
        .into_response();
    }
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminMailPollTriggerRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() || input.expected_state_version < 0 {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_mail_trigger",
            "邮件轮询触发参数无效",
        )
        .into_response();
    }
    let aggregate_id = admin_mail_aggregate_id(&input.mailbox_key);
    let request_hash =
        match RequestHash::from_request("admin_mail_trigger_poll", aggregate_id, &input) {
            Ok(hash) => hash,
            Err(_) => return super::internal_error(),
        };
    let Some(port) = state.admin_mail() else {
        return feature_unavailable();
    };
    let result = match port
        .trigger_poll(AdminMailPollTriggerCommand {
            command_id: CommandId::from_uuid(input.command_id),
            mailbox_key: input.mailbox_key.clone(),
            expected_state_version: input.expected_state_version,
            actor_id: identity.actor,
            request_id: None,
            request_hash,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate()
        || result.mailbox_key != input.mailbox_key
        || result.state_version != input.expected_state_version.saturating_add(1)
    {
        return super::internal_error();
    }
    (
        StatusCode::ACCEPTED,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "邮件轮询已触发",
            AdminMailStateResponse { polling: map_poll_state(result) },
        )),
    )
        .into_response()
}

fn parse_time(value: Option<&String>) -> Result<Option<DateTime<Utc>>, ()> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|_| ())
        })
        .transpose()
}

fn map_poll_state(state: AdminMailPollState) -> AdminMailPollingResponse {
    AdminMailPollingResponse {
        mailbox_key: state.mailbox_key,
        last_poll_at: super::timestamp(state.last_poll_at),
        next_poll_at: super::timestamp(state.next_poll_at),
        consecutive_failures: state.consecutive_failures,
        paused: state.paused,
        state_version: state.state_version,
        updated_at: super::timestamp(state.updated_at),
    }
}

fn map_summary(summary: AdminMailIntakeSummary) -> AdminMailIntakeSummaryResponse {
    AdminMailIntakeSummaryResponse {
        id: summary.id.to_string(),
        mailbox_key: summary.mailbox_key,
        external_mail_id: summary.external_mail_id,
        world_id: summary.world_id,
        kind: summary.kind,
        status: summary.status,
        decision_code: summary.decision_code,
        received_at: super::timestamp(summary.received_at),
        processed_at: super::timestamp(summary.processed_at),
    }
}

fn map_detail(detail: AdminMailIntakeDetail) -> AdminMailIntakeDetailResponse {
    let summary = map_summary(detail.summary);
    AdminMailIntakeDetailResponse {
        id: summary.id,
        mailbox_key: summary.mailbox_key,
        external_mail_id: summary.external_mail_id,
        world_id: summary.world_id,
        kind: summary.kind,
        status: summary.status,
        decision_code: summary.decision_code,
        received_at: summary.received_at,
        processed_at: summary.processed_at,
        metadata: map_metadata(detail.metadata),
    }
}

fn map_metadata(metadata: AdminMailIntakeMetadata) -> AdminMailIntakeMetadataResponse {
    AdminMailIntakeMetadataResponse {
        auction_id: metadata.auction_id,
        submission_id: metadata.submission_id,
        user_id: metadata.user_id,
        added_count: metadata.added_count,
        existing_count: metadata.existing_count,
        preview_revision: metadata.preview_revision,
        attachment_count: metadata.attachment_count,
        owner_result_code: metadata.owner_result_code,
        reason_code: metadata.reason_code,
        warning_codes: metadata.warning_codes,
        delivery_ids: metadata.delivery_ids,
    }
}

fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "Admin 邮件状态与入站审查仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

fn map_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminMail(AdminMailError::InvalidRequest)
        | ApplicationError::AdminMail(AdminMailError::InvalidCommand) => {
            super::api_error(StatusCode::BAD_REQUEST, "invalid_mail_request", "邮件参数无效")
                .into_response()
        }
        ApplicationError::AdminMail(AdminMailError::CommandConflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::AdminMail(AdminMailError::InvalidProjection)
        | ApplicationError::AdminMail(AdminMailError::StoredResultCorrupt) => {
            super::internal_error()
        }
        ApplicationError::AdminMailTransaction(AdminMailTransactionError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::AdminMailTransaction(AdminMailTransactionError::StateConflict {
            current_state_version,
        }) => (
            StatusCode::CONFLICT,
            Json(ApiErrorResponseWithData {
                code: "stale_state_version",
                message: "邮件轮询状态版本已变化".to_owned(),
                data: serde_json::json!({ "current_state_version": current_state_version }),
                request_id: None,
            }),
        )
            .into_response(),
        ApplicationError::AdminMailTransaction(AdminMailTransactionError::Conflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::Serialization(_) => super::internal_error(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "邮件服务暂不可用",
        )
        .into_response(),
    }
}
