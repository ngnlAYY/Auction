use std::net::SocketAddr;

use auction_application::{
    AdminMailIntakeDetail, AdminMailIntakeFilter, AdminMailIntakeMetadata, AdminMailIntakeSummary,
    ApplicationError, MailDisplaySnapshot,
};
use auction_contracts::{
    AdminMailDisplayResponse, AdminMailIntakeDetailResponse, AdminMailIntakeListResponse,
    AdminMailIntakeMetadataResponse, AdminMailIntakeSummaryResponse, AdminMailPaginationResponse,
    ApiResponse,
};
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{
    AdminApiState, admin_session_guard, authenticated_admin_session, feature_unavailable,
    internal_error, map_error, no_store_headers, not_found_error, parse_positive_i64, parse_query,
    timestamp,
};

/// 读取 Admin 入站邮件安全摘要列表。
pub(crate) async fn list_intakes(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query = match parse_query(
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
    let page = match parse_positive_i64(query.get("page"), 1, None) {
        Ok(page) => page,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page_size = match parse_positive_i64(
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
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
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
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
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
        return not_found_error().into_response();
    };
    if !result.summary.validate() || !result.metadata.validate() {
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("邮件入站详情已加载", map_detail(result))),
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

fn map_summary(summary: AdminMailIntakeSummary) -> AdminMailIntakeSummaryResponse {
    AdminMailIntakeSummaryResponse {
        id: summary.id.to_string(),
        mailbox_key: summary.mailbox_key,
        external_mail_id: summary.external_mail_id,
        world_id: summary.world_id,
        kind: summary.kind,
        status: summary.status,
        decision_code: summary.decision_code,
        received_at: timestamp(summary.received_at),
        processed_at: timestamp(summary.processed_at),
        sender: summary.sender,
        subject: summary.subject,
        has_message_display: summary.has_message_display,
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
        sender: summary.sender,
        subject: summary.subject,
        metadata: map_metadata(detail.metadata),
        message_display: detail.message_display.map(map_display),
    }
}

fn map_display(display: MailDisplaySnapshot) -> AdminMailDisplayResponse {
    AdminMailDisplayResponse {
        schema_version: display.schema_version,
        redaction_version: display.redaction_version,
        body: display.body,
        attachment_ids: display.attachment_ids,
        credits: display.credits,
        cod: display.cod,
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
