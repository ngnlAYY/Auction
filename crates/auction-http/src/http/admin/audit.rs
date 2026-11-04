//! Admin 审计 HTTP 适配器。
//! 这里负责严格解析旧契约的筛选和两种分页形状，并把应用层安全投影转换为稳定 DTO。审计
//! 记录只读，metadata 不在 HTTP 层重新解释为数据库模型。

use auction_application::diagnostics::DiagnosticError;

use crate::diagnostics;

use std::net::SocketAddr;

use auction_application::{
    AdminAuditCursor, AdminAuditListRequest, AdminAuditRecord, ApplicationError,
    MAX_ADMIN_AUDIT_OFFSET, MAX_ADMIN_AUDIT_PAGE_SIZE,
};
use auction_contracts::{
    AdminAuditCursorResponse, AdminAuditRecordResponse, AdminAuditResponse, ApiResponse,
};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;
const CURSOR_VERSION: u8 = 1;

/// 读取 Admin 审计记录。
pub(crate) async fn list(
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

    let mut query = match super::parse_query(
        &uri,
        &[
            "actor",
            "actor_type",
            "action",
            "entity",
            "source",
            "result",
            "request_id",
            "occurred_from",
            "occurred_to",
            "page",
            "page_size",
            "cursor",
            "before_time",
            "before_id",
        ],
    ) {
        Ok(query) => query,
        Err(_) => return invalid_query(),
    };
    let page_size = match super::parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_ADMIN_AUDIT_PAGE_SIZE),
    ) {
        Ok(value) => value,
        Err(_) => return invalid_query(),
    };
    let page = if query.contains_key("page") {
        match super::parse_positive_i64(query.get("page"), 1, None) {
            Ok(value) => Some(value),
            Err(_) => return invalid_query(),
        }
    } else {
        None
    };
    if page.is_some_and(|page| {
        page.checked_sub(1)
            .and_then(|value| value.checked_mul(page_size))
            .is_none_or(|offset| offset > MAX_ADMIN_AUDIT_OFFSET)
    }) {
        return invalid_query();
    }

    let has_before_time = query.contains_key("before_time");
    let has_before_id = query.contains_key("before_id");
    let raw_cursor = query.remove("cursor");
    if page.is_some() && (raw_cursor.is_some() || has_before_time || has_before_id)
        || raw_cursor.is_some() && (has_before_time || has_before_id)
        || has_before_time != has_before_id
    {
        return invalid_query();
    }
    let cursor = if let Some(value) = raw_cursor.as_deref() {
        match decode_cursor(value) {
            Ok(cursor) => Some(cursor),
            Err(()) => {
                return super::api_error(StatusCode::BAD_REQUEST, "invalid_cursor", "游标无效")
                    .into_response();
            }
        }
    } else if has_before_time {
        let occurred_at = match parse_time(query.get("before_time")) {
            Ok(value) => value,
            Err(()) => return invalid_query(),
        };
        let id = match query.get("before_id").and_then(|value| Uuid::parse_str(value).ok()) {
            Some(id) if !id.is_nil() => id,
            _ => return invalid_query(),
        };
        Some(AdminAuditCursor { occurred_at, id })
    } else {
        None
    };

    let request = AdminAuditListRequest {
        actor: optional_text(&mut query, "actor"),
        actor_type: optional_text(&mut query, "actor_type"),
        action: optional_text(&mut query, "action"),
        entity: optional_text(&mut query, "entity"),
        source: optional_text(&mut query, "source"),
        result: optional_text(&mut query, "result"),
        request_id: optional_text(&mut query, "request_id"),
        occurred_from: match parse_optional_time(&query, "occurred_from") {
            Ok(value) => value,
            Err(()) => return invalid_query(),
        },
        occurred_to: match parse_optional_time(&query, "occurred_to") {
            Ok(value) => value,
            Err(()) => return invalid_query(),
        },
        page,
        page_size,
        cursor,
    };
    if !request.validate() {
        return invalid_query();
    }
    let Some(port) = state.admin_audit() else {
        return feature_unavailable();
    };
    let result = match port.list(request.clone()).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate(&request) {
        return super::internal_error();
    }

    let has_more = if let Some(total) = result.total {
        request
            .page
            .and_then(|page| page.checked_mul(request.page_size))
            .is_some_and(|end| end < total)
    } else {
        result.has_more
    };
    let next_cursor_token = result.next_cursor.map(encode_cursor).unwrap_or_default();
    let response = AdminAuditResponse {
        records: result.entries.into_iter().map(map_record).collect(),
        page: result.page,
        page_size: result.page_size,
        total: result.total,
        has_more,
        next_cursor: result.next_cursor.map(map_cursor),
        next_cursor_token,
    };
    (StatusCode::OK, super::no_store_headers(), Json(ApiResponse::ok("审计记录已加载", response)))
        .into_response()
}

fn optional_text(
    query: &mut std::collections::BTreeMap<&str, String>,
    key: &str,
) -> Option<String> {
    query.remove(key).filter(|value| !value.trim().is_empty())
}

fn parse_optional_time(
    query: &std::collections::BTreeMap<&str, String>,
    key: &str,
) -> Result<Option<DateTime<Utc>>, ()> {
    let Some(value) = query.get(key) else {
        return Ok(None);
    };
    parse_time(Some(value)).map(Some)
}

fn parse_time(value: Option<&String>) -> Result<DateTime<Utc>, ()> {
    let value = value.ok_or(())?;
    DateTime::parse_from_rfc3339(value).map(|value| value.with_timezone(&Utc)).map_err(|_| ())
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CursorToken {
    v: u8,
    at: DateTime<Utc>,
    id: Uuid,
}

fn encode_cursor(cursor: AdminAuditCursor) -> String {
    let token = CursorToken { v: CURSOR_VERSION, at: cursor.occurred_at, id: cursor.id };
    let Ok(payload) = serde_json::to_vec(&token) else {
        return String::new();
    };
    URL_SAFE_NO_PAD.encode(payload)
}

fn decode_cursor(value: &str) -> Result<AdminAuditCursor, ()> {
    if value.is_empty() || value.len() > 512 {
        return Err(());
    }
    let payload = URL_SAFE_NO_PAD.decode(value).map_err(|_| ())?;
    let token: CursorToken = serde_json::from_slice(&payload).map_err(|_| ())?;
    if token.v != CURSOR_VERSION || token.id.is_nil() {
        return Err(());
    }
    let cursor = AdminAuditCursor { occurred_at: token.at, id: token.id };
    cursor.validate().then_some(cursor).ok_or(())
}

fn map_cursor(cursor: AdminAuditCursor) -> AdminAuditCursorResponse {
    AdminAuditCursorResponse {
        before_time: super::timestamp(cursor.occurred_at),
        before_id: cursor.id.to_string(),
    }
}

fn map_record(record: AdminAuditRecord) -> AdminAuditRecordResponse {
    AdminAuditRecordResponse {
        id: record.id.to_string(),
        occurred_at: super::timestamp(record.occurred_at),
        actor_type: record.actor_type,
        actor_id: record.actor_id,
        action: record.action,
        entity_type: record.entity_type,
        entity_id: record.entity_id,
        result: record.result,
        request_id: record.request_id,
        source: record.source,
        metadata: record.metadata,
    }
}

fn invalid_query() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response()
}

fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "Admin 审计查询需要已配置的数据库",
    )
    .into_response()
}

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::AdminAudit(_) => invalid_query(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Admin 审计服务暂不可用",
        )
        .into_response(),
    };
    let response = body();
    diagnostics::mapped_response("admin_audit", "respond_read_failure", diagnostic_kind, response)
}
