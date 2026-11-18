//! Admin 审计查询 HTTP 适配。
//! 页面只使用后端的安全审计投影；metadata 作为有界 JSON 展示，不在浏览器端推导状态或编辑
//! 历史事实。数字页语义与旧版保持一致，筛选变化会立即重新加载当前第一页。

use auction_contracts::{AdminAuditCursorResponse, ApiResponse};
pub(crate) use auction_contracts::{
    AdminAuditRecordResponse as AdminAuditRecord, AdminAuditResponse as AdminAuditPage,
};
use auction_time::{SecondsFormat, Timestamp};
use serde::Deserialize;
use url::form_urlencoded;

#[cfg(test)]
use super::realtime::send_get;
#[cfg(not(test))]
use super::send_get;
use super::{invalid_response_error, response_error, valid_admin_time, valid_uuid};

/// 旧 audit decoder 允许响应增加字段，但所有旧必需字段仍必须显式存在并满足业务语义。
/// 保留镜像的理由：wire 允许兼容扩展，contracts 的 `AdminAuditResponse`/
/// `AdminAuditRecordResponse`/`AdminAuditCursorResponse` 不能加 `deny_unknown_fields`；
/// 镜像继续要求旧字段显式存在（`next_cursor`/`request_id` 为必填 nullable）。同步义务：
/// 上述 DTO 字段集变化时必须同步本文件；decode 函数中构造 DTO 的穷举字段列表会在编译期
/// 捕捉漂移。
#[derive(Debug, Deserialize)]
struct StrictAuditPage {
    has_more: bool,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    next_cursor: Option<StrictAuditCursor>,
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
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    request_id: Option<String>,
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

impl AdminAuditFilter {
    /// 应用、恢复和发送筛选共用首尾空白规则，避免同一个精确匹配条件产生两个查询身份。
    pub(crate) fn normalize_text(&mut self) {
        for value in [
            &mut self.actor,
            &mut self.actor_type,
            &mut self.action,
            &mut self.entity,
            &mut self.source,
            &mut self.result,
            &mut self.request_id,
        ] {
            let trimmed = value.trim();
            if trimmed.len() != value.len() {
                *value = trimmed.to_owned();
            }
        }
    }

    /// 将本地时间或带偏移的旧值规范化，保证每个绝对时刻有唯一的筛选身份。
    pub(crate) fn absolute_time(value: &str) -> Option<String> {
        let value = value.trim();
        if value.is_empty() {
            return Some(String::new());
        }
        let instant = Timestamp::parse_from_rfc3339(value)
            .ok()
            .or_else(|| Timestamp::parse_from_rfc3339(&super::normalize_datetime(value)).ok())?;
        Some(instant.to_rfc3339_opts(SecondsFormat::AutoSi, true))
    }
}

/// 读取一页 Admin 审计安全投影。
pub(crate) async fn fetch_admin_audit(
    page: i64,
    mut filter: AdminAuditFilter,
    page_size: i64,
) -> Result<AdminAuditPage, String> {
    super::pagination::validate(page, page_size, 200, i64::from(i32::MAX))?;
    filter.normalize_text();
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
    append_time_filter(&mut query, "occurred_from", &filter.occurred_from)?;
    append_time_filter(&mut query, "occurred_to", &filter.occurred_to)?;

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
    if !super::pagination::valid_page_echo(
        requested_page,
        requested_size,
        data.page,
        data.page_size,
        data.total,
        data.records.len(),
    ) || data.has_more
        != data.page.checked_mul(data.page_size).is_some_and(|end| end < data.total)
        || data.next_cursor.is_none() && !data.next_cursor_token.is_empty()
    {
        return Err("audit_page");
    }
    let records =
        data.records.into_iter().map(decode_audit_record).collect::<Result<Vec<_>, _>>()?;
    let next_cursor = data.next_cursor.map(decode_audit_cursor).transpose()?;
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
        || !matches!(record.source.as_str(), "public" | "admin" | "worker" | "system" | "cli")
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
        request_id: record.request_id,
        source: record.source,
        metadata: record.metadata,
    })
}

fn append_filter(query: &mut form_urlencoded::Serializer<String>, key: &str, value: &str) {
    if !value.is_empty() {
        query.append_pair(key, value);
    }
}

fn append_time_filter(
    query: &mut form_urlencoded::Serializer<String>,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let absolute =
        AdminAuditFilter::absolute_time(value).ok_or_else(|| "invalid_query".to_owned())?;
    append_filter(query, key, &absolute);
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{StrictAuditPage, decode_audit_page};

    fn record() -> Value {
        json!({
            "id": "00000000-0000-4000-8000-000000000001",
            "occurred_at": "2026-11-14T00:00:00Z",
            "actor_type": "admin",
            "actor_id": "operator",
            "action": "auction.create",
            "entity_type": "auction",
            "entity_id": "00000000-0000-4000-8000-000000000002",
            "result": "success",
            "request_id": null,
            "source": "admin",
            "metadata": {}
        })
    }

    fn page(records: Vec<Value>) -> Value {
        json!({
            "has_more": false,
            "next_cursor": null,
            "next_cursor_token": "",
            "page": 1,
            "page_size": 20,
            "records": records,
            "total": 1
        })
    }

    fn without(mut value: Value, key: &str) -> Value {
        value.as_object_mut().expect("测试载荷必须是对象").remove(key);
        value
    }

    #[test]
    fn explicit_null_cursor_and_request_id_decode_as_none() {
        let data: StrictAuditPage =
            serde_json::from_value(page(vec![record()])).expect("显式 null");
        let decoded = decode_audit_page(data, 1, 20).expect("合法审计页");
        assert!(decoded.next_cursor.is_none());
        assert_eq!(decoded.records[0].request_id, None);
    }

    #[test]
    fn missing_nullable_audit_fields_fail_closed() {
        // 必填可空字段缺键与显式 null 不同：缺键说明 wire 形状漂移，必须拒绝。
        let missing_cursor = without(page(vec![record()]), "next_cursor");
        assert!(serde_json::from_value::<StrictAuditPage>(missing_cursor).is_err());
        let missing_request_id = page(vec![without(record(), "request_id")]);
        assert!(serde_json::from_value::<StrictAuditPage>(missing_request_id).is_err());
    }
}
