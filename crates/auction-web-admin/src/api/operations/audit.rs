//! Admin 审计查询 HTTP 适配。
//!
//! 页面只使用后端的安全审计投影；metadata 作为有界 JSON 展示，不在浏览器端推导状态或编辑
//! 历史事实。数字页语义与旧版保持一致，筛选变化会立即重新加载当前第一页。

use auction_contracts::ApiResponse;
pub(crate) use auction_contracts::{
    AdminAuditRecordResponse as AdminAuditRecord, AdminAuditResponse as AdminAuditPage,
};
use gloo_net::http::Request;
use url::form_urlencoded;

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
) -> Result<AdminAuditPage, String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.max(1).to_string());
    query.append_pair("page_size", "50");
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
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminAuditPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
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
