//! audit 相关 DTO。

use super::*;

/// Admin 审计记录安全投影；metadata 必须是安全 JSON 对象。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuditRecordResponse {
    pub id: String,
    pub occurred_at: String,
    pub actor_type: String,
    pub actor_id: String,
    pub action: String,
    pub entity_type: String,
    pub entity_id: String,
    pub result: String,
    pub request_id: Option<String>,
    pub source: String,
    pub metadata: Value,
}

/// Admin 审计 keyset 下一页边界。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuditCursorResponse {
    pub before_time: String,
    pub before_id: String,
}

/// Admin 审计查询响应；数字页与 keyset 页保留旧契约的互斥字段形状。
/// `page`/`total` 只在数字页出现；`next_cursor` 和 `next_cursor_token` 在两种页形状中
/// 都必须出现，即使当前页没有下一页。这样可以保留旧版客户端依赖的固定键集合，同时不
/// 影响应用层继续使用 opaque cursor。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuditResponse {
    pub records: Vec<AdminAuditRecordResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    pub page_size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    pub has_more: bool,
    pub next_cursor: Option<AdminAuditCursorResponse>,
    pub next_cursor_token: String,
}

/// Admin dashboard 的健康聚合。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDashboardHealthResponse {
    pub status: String,
    pub readiness: bool,
    /// 累计审计失败；发布进程为 classified 模式时只计阻断就绪的失败。
    pub audit_failure_count: i64,
    pub missing_services: Vec<String>,
}

/// Admin dashboard 的 Worker control 安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDashboardWorkerResponse {
    pub condition: String,
    pub state: String,
    pub ready: bool,
    pub version: String,
    pub mail_pollers: i64,
}

/// Admin dashboard section 的稳定外壳。
/// `data` 只允许由 owner 适配器构造安全 JSON；当 owner 未装配或读取失败时只返回
/// `status` 与有界 `error_code`，不返回数据库行或错误正文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDashboardSectionResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDashboardResponse {
    pub health: AdminDashboardHealthResponse,
    pub worker: AdminDashboardWorkerResponse,
    pub process: AdminDashboardSectionResponse,
    pub mail: AdminDashboardSectionResponse,
    pub realtime: AdminDashboardSectionResponse,
    pub credentials: AdminDashboardSectionResponse,
    /// 外部账号运行时快照；Worker control 不可用时为 unknown section。
    pub external_accounts: AdminDashboardSectionResponse,
    pub delivery: AdminDashboardSectionResponse,
    pub fulfillment: AdminDashboardSectionResponse,
    pub recovery: AdminDashboardSectionResponse,
    pub alerting: AdminDashboardSectionResponse,
    /// 资产对账与偿付能力报告投影；报告缺失时为 unknown section。
    pub reconciliation: AdminDashboardSectionResponse,
}
