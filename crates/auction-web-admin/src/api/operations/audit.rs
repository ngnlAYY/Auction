//! Admin 审计查询 HTTP 适配。
//! 页面只使用后端的安全审计投影；metadata 作为有界 JSON 展示，不在浏览器端推导状态或编辑
//! 历史事实。数字页语义与旧版保持一致，筛选变化会立即重新加载当前第一页。

use auction_contracts::{AdminAuditCursorResponse, ApiResponse};
pub(crate) use auction_contracts::{
    AdminAuditRecordResponse as AdminAuditRecord, AdminAuditResponse as AdminAuditPage,
};
use serde::Deserialize;
use url::form_urlencoded;

use super::{
    MAX_JSON_SAFE_INTEGER, RequiredOption, invalid_response_error, response_error, send_get,
    valid_admin_time, valid_uuid,
};

/// 旧 audit decoder 允许响应增加字段，但所有旧必需字段仍必须显式存在并满足业务语义。
/// 保留镜像的理由：wire 允许兼容扩展，contracts 的 `AdminAuditResponse`/
/// `AdminAuditRecordResponse`/`AdminAuditCursorResponse` 不能加 `deny_unknown_fields`；
/// 镜像继续要求旧字段显式存在（`next_cursor`/`request_id` 为必填 nullable）。同步义务：
/// 上述 DTO 字段集变化时必须同步本文件；decode 函数中构造 DTO 的穷举字段列表会在编译期
/// 捕捉漂移。
#[derive(Debug, Deserialize)]
struct StrictAuditPage {
    has_more: bool,
    next_cursor: RequiredOption<StrictAuditCursor>,
    next_cursor_token: String,
    page: i64,
    page_size: i64,
    records: Vec<StrictAuditRecord>,
    total: i64,
}

#[derive(Debug, Deserialize)]
struct StrictAuditCursor {
    before_id: String,
    before_time: String,
}

#[derive(Debug, Deserialize)]
struct StrictAuditRecord {
    action: String,
    actor_id: String,
    actor_type: String,
    entity_id: String,
    entity_type: String,
    id: String,
    metadata: serde_json::Value,
    occurred_at: String,
    request_id: RequiredOption<String>,
    result: String,
    source: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AdminAuditFilter {
    pub actor: String,
    pub actor_type: String,
    pub action: String,
    pub entity: String,
    pub source: String,
    pub result: String,
    pub request_id: String,
    pub occurred_from: String,
    pub occurred_to: String,
}

/// 读取一页 Admin 审计安全投影。
pub(crate) async fn fetch_admin_audit(
    page: i64,
    filter: AdminAuditFilter,
    page_size: i64,
) -> Result<AdminAuditPage, String> {
    super::pagination::validate(page, page_size, 200, i64::from(i32::MAX))?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.max(1).to_string());
    query.append_pair("page_size", &page_size.to_string());
    append_filter(&mut query, "actor", &filter.actor);
    append_filter(&mut query, "actor_type", &filter.actor_type);
    append_filter(&mut query, "action", &filter.action);
    append_filter(&mut query, "entity", &filter.entity);
    append_filter(&mut query, "source", &filter.source);
    append_filter(&mut query, "result", &filter.result);
    append_filter(&mut query, "request_id", &filter.request_id);
    append_time_filter(&mut query, "occurred_from", &filter.occurred_from);
    append_time_filter(&mut query, "occurred_to", &filter.occurred_to);

    let url = format!("/api/v1/admin/audit?{}", query.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictAuditPage> =
        response.json().await.map_err(invalid_response_error)?;
    decode_audit_page(payload.data, page, page_size).map_err(invalid_response_error)
}

fn decode_audit_page(
    data: StrictAuditPage,
    requested_page: i64,
    requested_size: i64,
) -> Result<AdminAuditPage, &'static str> {
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&data.page)
        || data.page != requested_page
        || data.page_size != requested_size
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&data.total)
        || i64::try_from(data.records.len())
            .map_or(true, |count| count > data.page_size || count > data.total)
        || data.has_more
            != data.page.checked_mul(data.page_size).is_some_and(|end| end < data.total)
        || data.next_cursor.0.is_none() && !data.next_cursor_token.is_empty()
    {
        return Err("audit_page");
    }
    let records =
        data.records.into_iter().map(decode_audit_record).collect::<Result<Vec<_>, _>>()?;
    let next_cursor = data.next_cursor.0.map(decode_audit_cursor).transpose()?;
    Ok(AdminAuditPage {
        records,
        page: Some(data.page),
        page_size: data.page_size,
        total: Some(data.total),
        has_more: data.has_more,
        next_cursor,
        next_cursor_token: data.next_cursor_token,
    })
}

fn decode_audit_cursor(
    cursor: StrictAuditCursor,
) -> Result<AdminAuditCursorResponse, &'static str> {
    if !valid_uuid(&cursor.before_id) || !valid_admin_time(&cursor.before_time) {
        return Err("audit_cursor");
    }
    Ok(AdminAuditCursorResponse { before_time: cursor.before_time, before_id: cursor.before_id })
}

fn decode_audit_record(record: StrictAuditRecord) -> Result<AdminAuditRecord, &'static str> {
    if !valid_uuid(&record.id)
        || !valid_admin_time(&record.occurred_at)
        || !matches!(record.actor_type.as_str(), "public_user" | "admin" | "worker" | "system")
        || record.actor_id.is_empty()
        || record.action.is_empty()
        || record.entity_type.is_empty()
        || record.entity_id.is_empty()
        || !matches!(record.result.as_str(), "success" | "rejected" | "failure")
        || !matches!(record.source.as_str(), "public" | "admin" | "worker" | "system")
        || !record.metadata.is_object()
    {
        return Err("audit_record");
    }
    Ok(AdminAuditRecord {
        id: record.id,
        occurred_at: record.occurred_at,
        actor_type: record.actor_type,
        actor_id: record.actor_id,
        action: record.action,
        entity_type: record.entity_type,
        entity_id: record.entity_id,
        result: record.result,
        request_id: record.request_id.0,
        source: record.source,
        metadata: record.metadata,
    })
}

fn append_filter(query: &mut form_urlencoded::Serializer<String>, key: &str, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        query.append_pair(key, value);
    }
}

fn append_time_filter(query: &mut form_urlencoded::Serializer<String>, key: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    let normalized = super::normalize_datetime(value);
    query.append_pair(key, &normalized);
}
