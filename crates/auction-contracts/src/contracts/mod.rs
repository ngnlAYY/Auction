use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// 旧系统统一响应外壳的成功形式。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct ApiResponse<T> {
    pub code: String,
    pub message: String,
    pub data: T,
}

impl<T> ApiResponse<T> {
    /// 构造稳定的成功响应；业务数据仍由 surface 自己定义。
    pub fn ok(message: impl Into<String>, data: T) -> Self {
        Self { code: "ok".to_owned(), message: message.into(), data }
    }

    /// 构造表示异步命令已接受的响应；它不表示外部副作用已经完成。
    pub fn accepted(message: impl Into<String>, data: T) -> Self {
        Self { code: "accepted".to_owned(), message: message.into(), data }
    }
}

/// 健康检查响应。
///
/// `service` 与 `version` 都由调用方传入：`env!("CARGO_PKG_VERSION")` 在本 crate 内展开只会
/// 得到 `auction-contracts` 自己的版本，而运维需要的是运行进程的版本。当前工作区版本统一，
/// 两者取值恰好相同，但语义不同——独立发布版本后差异就会显现。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct HealthResponse {
    pub service: String,
    pub status: String,
    pub version: String,
}

impl HealthResponse {
    /// 构造健康响应。`version` 应由二进制装配根传入自身的 `CARGO_PKG_VERSION`。
    pub fn ok(service: impl Into<String>, version: &'static str) -> Self {
        Self { service: service.into(), status: "ok".to_owned(), version: version.to_owned() }
    }
}

/// 与旧版 `/health/live`、`/health/ready` 保持一致的探针响应外壳。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct HealthProbeResponse<T> {
    pub code: String,
    pub message: String,
    pub data: T,
}

/// 进程存活探针数据。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct HealthLiveData {
    pub live: bool,
}

/// 进程就绪探针数据。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct HealthReadyData {
    pub ready: bool,
}

/// Public 持久化实时事件的安全信封。
///
/// 负载已经由应用层事件注册表校验；这里不暴露数据库内部状态版本列或 origin 字段。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicRealtimeEventResponse {
    pub event_id: String,
    pub stream_key: String,
    pub stream_sequence: i64,
    pub kind: String,
    pub schema_version: i32,
    pub payload: Value,
    pub created_at: String,
}

/// Public 事件回放响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicRealtimeResponse {
    pub events: Vec<PublicRealtimeEventResponse>,
    pub next_after: i64,
    pub has_more: bool,
    pub refetch_required: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct ApiErrorResponse {
    pub code: String,
    pub message: String,
    /// 可选的安全业务数据；只有明确冻结的错误契约可以填充。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    pub request_id: Option<String>,
}

/// 带稳定业务数据的错误响应，用于封禁原因等旧契约投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct ApiErrorResponseWithData<T> {
    pub code: String,
    pub message: String,
    pub data: T,
    pub request_id: Option<String>,
}

/// Public 普通竞价请求。命令 ID 必须由调用方生成并在重试中保持不变。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceBidRequest {
    pub command_id: Uuid,
    pub amount: i64,
}

/// Public 代理授权请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProxyAuthorizationRequest {
    pub command_id: Uuid,
    pub maximum_amount: i64,
    pub mode: String,
}

/// Public 降低代理上限请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LowerProxyAuthorizationRequest {
    pub command_id: Uuid,
    pub maximum_amount: i64,
}

/// 状态命令的最小确认数据，供前端区分接受、重放和冲突。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct CommandAcknowledgement {
    pub command_id: Uuid,
    pub result_code: String,
}

/// Admin 拍卖生命周期命令请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAuctionLifecycleRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Admin draft lot 修改请求；动作字段保持旧 HTTP surface 的字符串契约。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminLotUpdateRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_price: Option<i64>,
}

/// Admin 邮箱暂停/恢复轮询请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailPollingRequest {
    pub command_id: Uuid,
    pub mailbox_key: String,
    pub paused: bool,
    pub expected_state_version: i64,
}

/// Admin 立即触发一次邮箱轮询的请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailPollTriggerRequest {
    pub command_id: Uuid,
}

/// Admin 邮件轮询状态安全响应；不包含外部 Cookie 或原始游标。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailPollingResponse {
    pub mailbox_key: String,
    pub last_poll_at: String,
    pub next_poll_at: String,
    pub consecutive_failures: i32,
    pub paused: bool,
    pub state_version: i64,
    pub updated_at: String,
}

/// Admin 邮箱轮询状态响应 envelope。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailStateResponse {
    pub polling: AdminMailPollingResponse,
}

/// Admin 全局邮件轮询唤醒的接受结果。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailPollTriggerResponse {
    pub command_id: String,
    pub pollers: i64,
}

/// Admin 入站邮件列表安全摘要。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailIntakeSummaryResponse {
    pub id: String,
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub world_id: String,
    pub kind: String,
    pub status: String,
    pub decision_code: String,
    pub received_at: String,
    pub processed_at: String,
}

/// Admin 入站邮件数字分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 入站邮件列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailIntakeListResponse {
    pub intakes: Vec<AdminMailIntakeSummaryResponse>,
    pub pagination: AdminMailPaginationResponse,
}

/// Admin 入站邮件详情的固定元数据。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailIntakeMetadataResponse {
    pub auction_id: String,
    pub submission_id: String,
    pub user_id: String,
    pub added_count: i32,
    pub existing_count: i32,
    pub preview_revision: i64,
    pub attachment_count: i32,
    pub owner_result_code: String,
    pub reason_code: String,
    pub warning_codes: Vec<String>,
    pub delivery_ids: Vec<String>,
}

/// Admin 入站邮件详情响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailIntakeDetailResponse {
    pub id: String,
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub world_id: String,
    pub kind: String,
    pub status: String,
    pub decision_code: String,
    pub received_at: String,
    pub processed_at: String,
    pub metadata: AdminMailIntakeMetadataResponse,
}

/// Admin 拍卖生命周期命令的稳定结果投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuctionLifecycleResponse {
    pub command_id: Uuid,
    pub auction_id: String,
    pub status: String,
    pub state_version: u64,
    pub updated_at: String,
}

/// Admin 创建 draft 拍卖请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCreateAuctionRequest {
    pub command_id: Uuid,
    pub auction_id: Uuid,
    pub world_id: String,
    pub sequence: i64,
    pub preview_scheduled_at: DateTime<Utc>,
    pub scheduled_open_at: DateTime<Utc>,
    pub scheduled_end_at: DateTime<Utc>,
    pub expected_state_version: i64,
}

/// Admin 创建拍卖成功响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuctionCreateResponse {
    pub auction: AdminAuctionResponse,
}

/// Admin 装备录入请求。
///
/// `source_kind` 缺省时沿用旧契约的 URL 来源；URL 与 canonical JSON 必须二选一。装备 key
/// 只在 HTTPS 请求和应用适配器的短生命周期内使用，绝不进入成功响应。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminIntakeRequest {
    pub command_id: Uuid,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub equipment_url: Option<String>,
    #[serde(default)]
    pub equipment: Option<AdminCanonicalEquipmentRequest>,
    pub start_price: i64,
    pub player_id: String,
}

/// Admin canonical 装备输入 DTO；HTTP 边界保持字段闭集，避免把任意 JSON 透传到来源适配器。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCanonicalEquipmentRequest {
    pub external_equipment_id: String,
    pub name: String,
    pub category: String,
    pub equipment_type: String,
    pub quality: String,
    pub prefix: String,
    pub suffix: String,
    #[serde(default)]
    pub level: Option<i32>,
    pub tradeable: bool,
    pub stats: BTreeMap<String, AdminCanonicalEquipmentStatRequest>,
    pub key: String,
}

/// canonical 装备属性 DTO。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCanonicalEquipmentStatRequest {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<f64>,
}

impl std::fmt::Debug for AdminIntakeRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminIntakeRequest")
            .field("command_id", &self.command_id)
            .field("source_kind", &self.source_kind)
            .field("equipment_url", &self.equipment_url)
            .field("equipment", &self.equipment.as_ref().map(|_| "[REDACTED]"))
            .field("start_price", &self.start_price)
            .field("player_id", &self.player_id)
            .finish()
    }
}

impl std::fmt::Debug for AdminCanonicalEquipmentRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminCanonicalEquipmentRequest")
            .field("external_equipment_id", &self.external_equipment_id)
            .field("name", &self.name)
            .field("category", &self.category)
            .field("equipment_type", &self.equipment_type)
            .field("quality", &self.quality)
            .field("prefix", &self.prefix)
            .field("suffix", &self.suffix)
            .field("level", &self.level)
            .field("tradeable", &self.tradeable)
            .field("stats", &self.stats)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for AdminCanonicalEquipmentStatRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminCanonicalEquipmentStatRequest")
            .field("value", &self.value)
            .field("base", &self.base)
            .finish()
    }
}

impl Drop for AdminCanonicalEquipmentRequest {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.key).into_bytes();
        bytes.fill(0);
    }
}

/// Admin 装备录入成功响应；字段与旧版 `addIntakeItem` 契约保持一致。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminIntakeResponse {
    pub auction_id: Uuid,
    pub auction_sequence: i64,
    pub auction_state_version: i64,
    pub preview_revision: i64,
    pub submission_id: Uuid,
    pub lot_id: Uuid,
    pub lot_number: i32,
    pub lot_state_version: i64,
    pub equipment_id: Uuid,
    pub external_equipment_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_level: Option<i32>,
    pub start_price: i64,
    pub seller_player_id: String,
}

/// Admin 规则候选校验请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRulesCandidateRequest {
    pub locale: String,
    pub title: String,
    pub markdown: String,
}

/// Admin 规则发布请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRulesPublishRequest {
    pub command_id: Uuid,
    pub expected_revision: i64,
    pub locale: String,
    pub title: String,
    pub markdown: String,
}

/// Admin runtime 配置候选校验请求；文档值由应用层反序列化为严格的非敏感 TOML 模型。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRuntimeConfigValidateRequest {
    pub document: Value,
}

/// Admin runtime 配置更新请求；fingerprint 是当前文件快照的版本 fence。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRuntimeConfigUpdateRequest {
    pub command_id: Uuid,
    pub expected_fingerprint: String,
    pub document: Value,
}

/// Admin 改名历史安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRenameResponse {
    pub id: String,
    pub user_id: String,
    pub user_state_version: i64,
    pub previous_username: String,
    pub next_username: String,
    pub actor: String,
    pub created_at: String,
    pub reverted_at: Option<String>,
}

/// Admin 改名历史数字分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRenamePaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 改名历史列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRenameListResponse {
    pub renames: Vec<AdminRenameResponse>,
    pub pagination: AdminRenamePaginationResponse,
}

/// Admin recovery 确认请求；`expected_state_version` 是报告快照中的版本 fence。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRecoveryConfirmationRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
    pub action: String,
    #[serde(default)]
    pub reason: String,
}

/// Admin recovery 确认结果的稳定投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRecoveryConfirmationResultResponse {
    pub recovery_id: String,
    pub action: String,
    pub stage: String,
    pub resolution: String,
    pub state_version: i64,
    pub relisted_auction_id: Option<String>,
    pub relisted_lot_id: Option<String>,
}

/// Admin recovery 确认成功响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRecoveryConfirmationResponse {
    pub recovery: AdminRecoveryConfirmationResultResponse,
}

/// Admin delivery 安全投影；不包含 payload、lease token 或原始 result metadata。
/// `cod` 只表示经过 owner 校验后从版本化 send payload 提取的金额；其余三个结果字段同样
/// 由 owner 从版本化 metadata 派生，不把 metadata 原文传给浏览器。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminDeliveryResponse {
    pub id: String,
    pub origin_kind: String,
    pub origin_id: String,
    pub recipient: String,
    pub cod: Option<i64>,
    pub status: String,
    pub lease_expires_at: Option<String>,
    pub fencing_version: i64,
    pub attempt_count: u32,
    pub claimed_at: Option<String>,
    pub sending_at: Option<String>,
    pub completed_at: Option<String>,
    pub external_reference: Option<String>,
    pub failure_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub take_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returned_mail_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Admin delivery 列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminDeliveryListResponse {
    pub deliveries: Vec<AdminDeliveryResponse>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin delivery 状态命令请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryCommandRequest {
    pub command_id: Uuid,
    pub expected_fencing_version: i64,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub external_reference: Option<String>,
    #[serde(default)]
    pub failure_code: Option<String>,
}

/// Admin delivery 状态命令响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminDeliveryCommandResponse {
    pub delivery: AdminDeliveryResponse,
}

/// Admin realtime lag 的安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeLagResponse {
    pub stream_key: String,
    pub consumer_key: String,
    pub offset: i64,
    pub latest_sequence: i64,
    pub lag: i64,
}

/// Admin realtime lag 列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeLagListResponse {
    pub lags: Vec<AdminRealtimeLagResponse>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin realtime failure 的安全投影；不包含原始驱动错误或未过滤 payload。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeFailureResponse {
    pub id: String,
    pub event_id: String,
    pub stream_key: String,
    pub consumer_key: String,
    pub code: String,
    pub safe_detail: String,
    pub failed_at: String,
    pub resolved_at: Option<String>,
    pub resolved: bool,
}

/// Admin realtime failure 列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeFailureListResponse {
    pub failures: Vec<AdminRealtimeFailureResponse>,
    pub page: i64,
    pub page_size: i64,
    pub has_more: bool,
    pub total: i64,
}

/// Admin realtime failure 详情响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeFailureDetailResponse {
    pub failure: AdminRealtimeFailureResponse,
}

/// Admin realtime failure replay 请求。
///
/// `command_id` 必须在重试中保持不变；payload 只允许应用事件注册表接受的公开对象。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRealtimeReplayRequest {
    pub command_id: Uuid,
    pub schema_version: i32,
    pub public_payload: Value,
}

/// Admin realtime replay 追加事件的安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeReplayEventResponse {
    pub event_id: String,
    pub replay_of_event_id: String,
    pub stream_key: String,
    pub stream_sequence: i64,
    pub kind: String,
    pub schema_version: i32,
    pub created_at: String,
}

/// Admin realtime replay 响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeReplayResponse {
    pub replay: AdminRealtimeReplayEventResponse,
}

/// Admin 结算报告中的拍卖生命周期投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminSettlementAuctionResponse {
    pub id: String,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub preview_scheduled_at: String,
    pub scheduled_open_at: String,
    pub preview_opened_at: Option<String>,
    pub preview_manual_open: bool,
    pub preview_revision: i64,
    pub opened_at: Option<String>,
    pub final_phase_at: Option<String>,
    pub settled_at: Option<String>,
    pub cancelled_at: Option<String>,
    pub cancelled_reason: Option<String>,
    pub cancelled_by: Option<String>,
    pub state_version: i64,
    pub scheduled_end_at: Option<String>,
    pub effective_end_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Admin 拍卖列表的能力声明。
///
/// 能力只表示对应命令路由已经存在；未声明的扩展命令仍保持明确的未迁移响应，不会由能力
/// 字段推断出额外的业务 owner。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuctionCapabilitiesResponse {
    pub cancel_auction: bool,
    pub manual_early_open: bool,
    pub manual_intake: bool,
    pub manual_preview_open: bool,
}

/// Admin 拍卖列表中的生命周期安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuctionResponse {
    pub id: String,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub preview_scheduled_at: String,
    pub scheduled_open_at: String,
    pub preview_opened_at: Option<String>,
    pub preview_manual_open: bool,
    pub preview_revision: i64,
    pub opened_at: Option<String>,
    pub final_phase_at: Option<String>,
    pub settled_at: Option<String>,
    pub cancelled_at: Option<String>,
    pub cancelled_reason: Option<String>,
    pub cancelled_by: Option<String>,
    pub state_version: i64,
    pub scheduled_end_at: Option<String>,
    pub effective_end_at: Option<String>,
    pub scheduled_ends_at: Option<String>,
    pub lot_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Admin 拍卖列表分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuctionPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 拍卖列表响应；列表和 lot 数量来自同一 PostgreSQL 快照。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuctionListResponse {
    pub auctions: Vec<AdminAuctionResponse>,
    pub capabilities: AdminAuctionCapabilitiesResponse,
    pub pagination: AdminAuctionPaginationResponse,
}

/// Admin 结算摘要响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminSettlementSummaryResponse {
    pub opening_balance: i64,
    pub tax_income: i64,
    pub discount_demand: i64,
    pub compensation_out: i64,
    pub closing_balance: i64,
    pub movement_count: i64,
    pub settled_at: String,
}

/// Admin 结算 lot 的 adjustment metadata。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminSettlementAdjustmentResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
}

/// Admin 结算报告中的单个 lot。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminSettlementLotResponse {
    pub id: String,
    pub lot_number: i32,
    pub status: String,
    pub equipment_name: String,
    pub seller: Option<String>,
    pub winner: Option<String>,
    pub final_price: i64,
    pub buyer_amount: i64,
    pub seller_amount: i64,
    pub tax_amount: i64,
    pub compensation_amount: i64,
    pub adjustment: AdminSettlementAdjustmentResponse,
}

/// Admin 结算报告响应；未结算拍卖的 `settlement` 为 null，且不伪造 lot 金额。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminSettlementReportResponse {
    pub auction: AdminSettlementAuctionResponse,
    pub settlement: Option<AdminSettlementSummaryResponse>,
    pub lots: Vec<AdminSettlementLotResponse>,
}

/// Admin lot 的安全投影；不包含装备密钥、代理上限或原始外部正文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminLotResponse {
    pub id: String,
    pub auction_id: String,
    pub equipment_id: String,
    pub submission_id: Option<String>,
    pub lot_number: i32,
    pub status: String,
    pub start_price: i64,
    pub current_price: i64,
    pub leader_user_id: Option<String>,
    pub effective_end_at: Option<String>,
    pub slow_due_at: Option<String>,
    pub slow_generation: i64,
    pub slow_authorization_id: Option<String>,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub external_equipment_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_attributes: Value,
    pub equipment_collected_at: String,
    pub seller: Option<String>,
    pub submission_source_kind: Option<String>,
    pub submission_source_reference: Option<String>,
    pub mailbox_key: Option<String>,
    pub external_mail_id: Option<String>,
    pub submission_received_at: Option<String>,
}

/// Admin draft lot 修改成功响应；返回同一事务快照中的 lot 安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminLotUpdateResponse {
    pub lot: AdminLotResponse,
}

/// Admin lot 列表分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminLotPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin lot 页面可用的外部写入能力；浏览器不得根据数据库档位自行猜测。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminLotCapabilitiesResponse {
    pub manual_intake: bool,
}

/// Admin lot 列表响应；拍卖和 lot 来自同一数据库快照。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminLotListResponse {
    pub auction: AdminSettlementAuctionResponse,
    pub lots: Vec<AdminLotResponse>,
    pub pagination: AdminLotPaginationResponse,
    pub capabilities: AdminLotCapabilitiesResponse,
}

/// Admin 代理历史的安全事件投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminProxyEventResponse {
    pub authorization_id: String,
    pub authorization_sequence: i64,
    pub command_id: String,
    pub created_at: String,
    pub lot_id: String,
    pub maximum_amount: i64,
    pub mode: String,
    pub state_version: i64,
    pub status: String,
    pub user_id: String,
    pub usage_number: i64,
    pub username: String,
}

/// Admin 代理历史的页码或游标分页信息。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminProxyEventPaginationResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    pub page_size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

/// Admin 代理历史响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminProxyEventListResponse {
    pub proxy_events: Vec<AdminProxyEventResponse>,
    pub pagination: AdminProxyEventPaginationResponse,
}

/// Admin recovery action 的安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRecoveryActionResponse {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub delivery_id: Option<String>,
    pub failure_code: Option<String>,
    pub proof_kind: Option<String>,
    pub proof_observed_at: Option<String>,
    pub updated_at: String,
}

/// Admin recovery case 的安全投影，不包含内部 claim/lease 或外部正文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRecoveryResponse {
    pub id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_number: i32,
    pub world_id: String,
    pub equipment_name: String,
    pub stage: String,
    pub resolution: Option<String>,
    pub state_version: i64,
    pub payment_deadline_at: String,
    pub reminder_count: i32,
    pub reminder_limit: i32,
    pub custody_proof_kind: Option<String>,
    pub custody_proof_observed_at: Option<String>,
    pub blocking_code: Option<String>,
    pub manual_reconcile_delivery_id: Option<String>,
    pub relisted_auction_id: Option<String>,
    pub relisted_lot_id: Option<String>,
    pub actions: Vec<AdminRecoveryActionResponse>,
    pub allowed_commands: Vec<String>,
    pub updated_at: String,
}

/// Admin recovery 报告响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRecoveryReportResponse {
    pub auction_id: String,
    pub recoveries: Vec<AdminRecoveryResponse>,
}

/// Public 普通竞价成功后的稳定结果。
///
/// 结果来自竞价事务提交后的 lot 投影；重放同一 `command_id` 时返回同一份投影，不把数据库
/// 计划或内部竞价事实直接暴露给浏览器。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PlaceBidResponse {
    pub bid: PlaceBidResultResponse,
}

/// Public 普通竞价结果中的公开 lot 状态。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PlaceBidResultResponse {
    pub lot_id: String,
    pub current_price: i64,
    pub effective_end_at: String,
    pub state_version: i64,
    pub leading: bool,
}

/// 当前用户竞价事实的账户投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedBidResponse {
    pub id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_number: i64,
    pub amount: i64,
    pub accepted_state_version: i64,
    pub fact_index: i32,
    pub mechanism: String,
    pub created_at: String,
    pub leading: bool,
    pub equipment: PublicOwnedEquipmentResponse,
}

/// 账户投影中的最小装备信息。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedEquipmentResponse {
    pub name: String,
    pub category: String,
}

/// 账户代理授权 quota。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedProxyQuotaResponse {
    pub used: i64,
    pub limit: i64,
}

/// 账户代理授权的历史转换事实。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedProxyTransitionResponse {
    pub transition_number: i64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_maximum_amount: Option<i64>,
    pub new_maximum_amount: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_mode: Option<String>,
    pub new_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_status: Option<String>,
    pub new_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub resulting_state_version: i64,
    pub created_at: String,
}

/// 账户代理授权当前可执行能力。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedProxyCapabilitiesResponse {
    pub can_lower: bool,
    pub waiting: bool,
    pub at_maximum: bool,
}

/// 当前用户自己的代理授权投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedProxyResponse {
    pub authorization_id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_number: i64,
    pub authorization_sequence: i64,
    pub usage_number: i64,
    pub mode: String,
    pub maximum_amount: i64,
    pub quota: PublicOwnedProxyQuotaResponse,
    pub transitions: Vec<PublicOwnedProxyTransitionResponse>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub equipment: PublicOwnedEquipmentResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bid: Option<PublicOwnedBidResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
    pub capabilities: PublicOwnedProxyCapabilitiesResponse,
}

/// Public 代理命令刚刚产生的竞价结果。
///
/// 该 DTO 对齐旧系统命令响应的最小公开字段；它不携带账户页专属的拍卖、装备、时间和
/// capability 字段，也不把命令账本的原始结果暴露给浏览器。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicProxyCommandBidResponse {
    pub lot_id: String,
    pub current_price: i64,
    pub effective_end_at: String,
    pub state_version: i64,
    pub leading: bool,
}

/// Public 代理命令成功后的授权投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicProxyCommandAuthorizationResponse {
    pub authorization_id: String,
    pub lot_id: String,
    pub authorization_sequence: i64,
    pub usage_number: i64,
    pub mode: String,
    pub maximum_amount: i64,
    pub quota: PublicOwnedProxyQuotaResponse,
    pub transitions: Vec<PublicOwnedProxyTransitionResponse>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    pub state_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bid: Option<PublicProxyCommandBidResponse>,
}

/// Public 代理创建/降低命令响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicProxyCommandResponse {
    pub authorization: PublicProxyCommandAuthorizationResponse,
}

/// 当前用户竞价与代理历史列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicMyBidsResponse {
    pub bids: Vec<PublicOwnedBidResponse>,
    pub proxies: Vec<PublicOwnedProxyResponse>,
    pub bid_total: i64,
    pub proxy_total: i64,
    pub page: i64,
    pub bid_page: i64,
    pub proxy_page: i64,
    pub page_size: i64,
}

/// 当前用户在一个 lot 上的代理查询响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicMyProxyResponse {
    pub authorization: Option<PublicOwnedProxyResponse>,
}

/// Public 登录请求；原始 key 只在 HTTP 到应用端口的短路径中存在。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicLoginRequest {
    pub key: String,
}

/// Public 登录成功响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicLoginResponse {
    pub user_id: String,
    pub username: String,
    pub status: String,
    pub expires_at: String,
    pub absolute_expires_at: String,
}

/// Public 当前会话响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicSessionCurrentResponse {
    pub authenticated: bool,
    pub user_id: String,
    pub username: String,
    pub expires_at: String,
    pub absolute_expires_at: String,
}

/// Admin 登录或当前会话响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminSessionResponse {
    pub authenticated: bool,
    pub actor: String,
    pub csrf_token: String,
    pub expires_at: String,
    pub absolute_expires_at: String,
}

/// Public 黑名单响应中的可公开原因。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicAccountBlockedResponse {
    pub public_reason: String,
}

/// Public 世界条目的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct WorldStatusResponse {
    pub enabled: bool,
}

/// Public 世界目录响应，保持旧系统以世界 ID 为 key 的 JSON 结构。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct WorldsResponse {
    pub worlds: BTreeMap<String, WorldStatusResponse>,
}

/// Public 规则文档响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicRulesResponse {
    pub locale: String,
    pub title: String,
    pub markdown: String,
    pub revision: i64,
    pub published_at: String,
}

/// Admin 规则文档安全响应；包含发布者，但不包含数据库内部模型。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRulesResponse {
    pub locale: String,
    pub title: String,
    pub markdown: String,
    pub published_by: String,
    pub revision: i64,
    pub published_at: String,
}

/// Admin 规则候选校验响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRulesCandidateResponse {
    pub locale: String,
    pub title: String,
    pub markdown: String,
}

/// Admin runtime 配置安全响应；更新成功仍明确要求重启才能让进程读取新文档。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRuntimeConfigResponse {
    pub document: Value,
    pub fingerprint: String,
    pub restart_required: bool,
}

/// Public 黑名单条目的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicBlacklistEntryResponse {
    pub game_player_id: String,
    pub reason: String,
    pub blacklisted_at: String,
}

/// Public 黑名单分页响应，保持旧系统的 `users` 字段命名。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicBlacklistResponse {
    pub users: Vec<PublicBlacklistEntryResponse>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

/// Public 拍卖历史列表中的稳定摘要。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AuctionSummaryResponse {
    pub id: String,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub scheduled_open_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_phase_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_end_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
    pub lot_count: i64,
    pub sold_count: i64,
    pub unsold_count: i64,
    pub sold_amount: i64,
    pub state_version: i64,
}

/// Public 历史拍卖列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AuctionListResponse {
    pub auctions: Vec<AuctionSummaryResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    pub page_size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

/// Public 竞价日志条目的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicBidLogEntryResponse {
    pub id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_number: i64,
    pub amount: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    pub bidder_name: String,
    pub valid: bool,
    pub accepted_state_version: i64,
    pub fact_index: i32,
    pub mechanism: String,
    pub created_at: String,
}

/// Public 竞价日志列表响应；编号页与 cursor 页沿用旧系统互斥字段形状。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicBidLogListResponse {
    pub bids: Vec<PublicBidLogEntryResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    pub page_size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

/// Public 价格历史条目的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPriceHistoryEntryResponse {
    pub lot_id: String,
    pub auction_id: String,
    pub world_id: String,
    pub auction_sequence: i64,
    pub lot_number: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_level: Option<i64>,
    pub buyer_name: String,
    pub seller_name: String,
    pub price: i64,
    pub closed_at: String,
}

/// Public 价格历史编号页的完整过滤集统计。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPriceHistoryStatsResponse {
    pub count: i64,
    pub min: i64,
    pub max: i64,
    pub median: i64,
}

/// Public 价格历史响应；cursor 页不携带 stats/total。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPriceHistoryListResponse {
    pub rows: Vec<PublicPriceHistoryEntryResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<PublicPriceHistoryStatsResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    pub page_size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

/// Public 补偿池批次的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationRoundResponse {
    pub auction_id: String,
    pub world_id: String,
    pub auction_sequence: i64,
    pub opening_balance: i64,
    pub tax_income: i64,
    pub discount_demand: i64,
    pub compensation_out: i64,
    pub closing_balance: i64,
    pub movement_count: i64,
    pub settled_at: String,
}

/// Public 补偿池账本 movement 的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationMovementResponse {
    pub sequence: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lot_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_category: Option<String>,
    pub kind: String,
    pub amount: i64,
    pub balance_after: i64,
    pub created_at: String,
}

/// Public 补偿池批次列表响应；编号页与 cursor 页字段互斥。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationResponse {
    pub balance: i64,
    pub updated_at: Option<String>,
    pub rounds: Vec<PublicCompensationRoundResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    pub page_size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

/// Public 单场补偿账本响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationMovementPageResponse {
    pub round: PublicCompensationRoundResponse,
    pub movements: Vec<PublicCompensationMovementResponse>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

/// Public 论坛镜像同步状态响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicForumResponse {
    pub auction_id: String,
    pub status: String,
    pub source_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub updated_at: String,
}

/// 当前拍卖中的公开上下文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct CurrentAuctionResponse {
    pub auction: Option<CurrentAuctionItemResponse>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct CurrentAuctionItemResponse {
    pub id: String,
    pub world: String,
    pub number: i64,
    pub status: String,
    pub opens_at: String,
    pub scheduled_ends_at: Option<String>,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub preview_revision: i64,
}

/// lot 列表与详情共用的公开拍卖上下文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicLotProjectionResponse {
    pub auction_id: String,
    pub auction_status: String,
    pub projection_kind: String,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub opens_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_ends_at: Option<String>,
    pub preview_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_restriction: Option<PublicFinalRestrictionResponse>,
}

/// 终阶段代理限制的公开契约。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicFinalRestrictionResponse {
    pub new_slow_proxy_allowed: bool,
    pub existing_slow_proxy_converted: bool,
}

/// 公开装备 stat 的稳定 JSON 形状。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicEquipmentStatResponse {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roll: Option<f64>,
}

/// 公开装备详情；attributes 仅由 lot 详情显式传入。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicEquipmentResponse {
    pub name: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub info_summary: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub stats: BTreeMap<String, PublicEquipmentStatResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_roll: Option<f64>,
    pub game_equipment_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<serde_json::Value>,
}

/// 正式竞拍 lot 的公开契约。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicLiveLotResponse {
    pub id: String,
    pub auction_id: String,
    pub lot_number: i64,
    pub status: String,
    pub start_price: i64,
    pub current_price: i64,
    pub next_bid: i64,
    pub has_leader: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader_name: Option<String>,
    pub seller_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_end_at: Option<String>,
    pub state_version: i64,
    pub equipment: PublicEquipmentResponse,
}

/// 预览 lot 的公开契约。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPreviewLotResponse {
    pub id: String,
    pub auction_id: String,
    pub code: String,
    pub status: String,
    pub seller_name: String,
    pub start_price: i64,
    pub equipment_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub equipment_category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_level: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_roll: Option<f64>,
    pub game_equipment_url: String,
}

/// 预览 lot 详情沿用摘要字段，并附加完整公开装备投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPreviewLotDetailResponse {
    #[serde(flatten)]
    pub summary: PublicPreviewLotResponse,
    pub equipment: PublicEquipmentResponse,
}

/// lot 列表的无标签数组，保持旧 JSON 契约的 preview/live 形状。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PublicLotListItemsResponse {
    Preview(Vec<PublicPreviewLotResponse>),
    Live(Vec<PublicLiveLotResponse>),
}

/// lot 详情的无标签对象，保持旧 JSON 契约的 preview/live 形状。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PublicLotDetailItemResponse {
    Preview(PublicPreviewLotDetailResponse),
    Live(PublicLiveLotResponse),
}

/// Public lot 列表与详情共用的响应外壳。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicLotEnvelopeResponse {
    pub auction_id: String,
    pub world: String,
    pub number: i64,
    pub auction_status: String,
    pub projection_kind: String,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub opens_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_ends_at: Option<String>,
    pub preview_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_restriction: Option<PublicFinalRestrictionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lots: Option<PublicLotListItemsResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lot: Option<PublicLotDetailItemResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_counts: Option<BTreeMap<String, i64>>,
}

/// Admin 论坛预览请求；HTML 与纯文本必须恰好提供一个。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminForumPreviewRequest {
    pub html: Option<String>,
    pub text: Option<String>,
}

impl std::fmt::Debug for AdminForumPreviewRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminForumPreviewRequest")
            .field("html_bytes", &self.html.as_ref().map(String::len))
            .field("text_bytes", &self.text.as_ref().map(String::len))
            .finish()
    }
}

/// Admin 论坛预览中的 lot。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumLotResponse {
    pub code: String,
    pub category: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seller: Option<String>,
    pub start_price: i64,
    pub material: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_bidder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_price: Option<i64>,
}

/// Admin 论坛预览目录。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumCatalogResponse {
    pub categories: Vec<String>,
    pub lots: Vec<AdminForumLotResponse>,
    pub warnings: Vec<AdminForumWarningResponse>,
}

/// Admin 论坛预览帖子。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumPostResponse {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub text: String,
    pub edited: bool,
}

/// Admin 论坛预览命令。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumCommandResponse {
    pub line: usize,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub lot_codes: Vec<String>,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub proxy_lower: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify: Option<String>,
}

/// Admin 论坛预览帖子命令集合。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumBidPostResponse {
    pub post: AdminForumPostResponse,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<AdminForumCommandResponse>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<AdminForumWarningResponse>,
    pub ignored: bool,
}

/// Admin 论坛预览警告。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumWarningResponse {
    pub line: usize,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_id: Option<String>,
}

/// Admin 论坛拍卖预览成功响应数据。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumPreviewResponse {
    pub catalog: AdminForumCatalogResponse,
    pub posts: Vec<AdminForumBidPostResponse>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<AdminForumWarningResponse>,
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
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
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuditResponse {
    pub records: Vec<AdminAuditRecordResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    pub page_size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<AdminAuditCursorResponse>,
    pub next_cursor_token: String,
}

/// Admin dashboard 的健康聚合。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminDashboardHealthResponse {
    pub status: String,
    pub readiness: bool,
    pub audit_failure_count: i64,
    pub missing_services: Vec<String>,
}

/// Admin dashboard 的 Worker control 安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminDashboardWorkerResponse {
    pub condition: String,
    pub state: String,
    pub ready: bool,
    pub version: String,
    pub mail_pollers: i64,
}

/// Admin dashboard section 的稳定外壳。
///
/// `data` 只允许由 owner 适配器构造安全 JSON；当 owner 尚未迁移或读取失败时只返回
/// `status` 与有界 `error_code`，不返回数据库行或错误正文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminDashboardSectionResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Admin dashboard 响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminDashboardResponse {
    pub health: AdminDashboardHealthResponse,
    pub worker: AdminDashboardWorkerResponse,
    pub process: AdminDashboardSectionResponse,
    pub mail: AdminDashboardSectionResponse,
    pub realtime: AdminDashboardSectionResponse,
    pub credentials: AdminDashboardSectionResponse,
    pub delivery: AdminDashboardSectionResponse,
    pub fulfillment: AdminDashboardSectionResponse,
    pub recovery: AdminDashboardSectionResponse,
    pub alerting: AdminDashboardSectionResponse,
}

/// Admin 用户创建请求；`key` 仅在手工 key 模式提供，服务端不会把它写入日志或命令账本。
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserCreateRequest {
    pub command_id: Uuid,
    pub username: String,
    pub game_player_id: String,
    pub key_mode: String,
    pub key: Option<String>,
}

impl std::fmt::Debug for AdminUserCreateRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminUserCreateRequest")
            .field("command_id", &self.command_id)
            .field("username", &self.username)
            .field("game_player_id", &self.game_player_id)
            .field("key_mode", &self.key_mode)
            .field("key", &self.key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Admin 用户属性/key 状态修改请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserMutationRequest {
    pub command_id: Uuid,
    pub user_id: Uuid,
    pub expected_state_version: i64,
    pub action: String,
    pub username: Option<String>,
    pub game_player_id: Option<String>,
    pub public_reason: Option<String>,
}

/// Admin 用户黑名单请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserBlacklistRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
    pub public_reason: String,
}

/// Admin 用户解封请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserUnblacklistRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
}

/// Admin 改名回滚请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRenameRevertRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
}

/// Admin 用户安全响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserResponse {
    pub id: String,
    pub username: String,
    pub game_player_id: Option<String>,
    pub status: String,
    pub key_status: String,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Admin 用户列表分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 用户列表中的黑名单历史安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserBlacklistHistoryResponse {
    pub public_reason: String,
    pub blacklisted_at: String,
}

/// Admin 用户列表条目；写命令响应仍只使用 `AdminUserResponse`。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserListItemResponse {
    #[serde(flatten)]
    pub user: AdminUserResponse,
    pub blacklist_count: i64,
    pub blacklist_history: Vec<AdminUserBlacklistHistoryResponse>,
}

/// Admin 用户列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserListResponse {
    pub users: Vec<AdminUserListItemResponse>,
    pub pagination: AdminUserPaginationResponse,
}

/// Admin 用户修改响应；生成 key 只在本次创建/重签响应中出现。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserMutationResponse {
    pub user: AdminUserResponse,
    pub generated_key: Option<String>,
}

/// Admin 凭据导入请求；secret 只在请求体和本次加密调用中存在，Debug 实现必须脱敏。
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialCreateRequest {
    pub command_id: Uuid,
    pub kind: String,
    pub world: String,
    pub label: String,
    pub secret: String,
}

impl std::fmt::Debug for AdminCredentialCreateRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminCredentialCreateRequest")
            .field("command_id", &self.command_id)
            .field("kind", &self.kind)
            .field("world", &self.world)
            .field("label", &self.label)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

/// Admin 凭据改名或状态修改请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialMutationRequest {
    pub command_id: Uuid,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub action: String,
    pub label: Option<String>,
    pub status: Option<String>,
}

/// Admin 凭据删除请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialDeleteRequest {
    pub command_id: Uuid,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
}

/// Admin 凭据安全响应，不包含 secret、密文、密钥标识或指纹。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminCredentialResponse {
    pub id: String,
    pub kind: String,
    pub world: String,
    pub label: String,
    pub status: String,
    pub state_version: i64,
    pub created_at: String,
    pub rotated_at: Option<String>,
    pub updated_at: String,
}

/// Admin 凭据列表分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminCredentialPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 凭据列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminCredentialListResponse {
    pub credentials: Vec<AdminCredentialResponse>,
    pub pagination: AdminCredentialPaginationResponse,
    pub restart_required: bool,
}

/// Admin 凭据导入/修改响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminCredentialMutationResponse {
    pub credential: AdminCredentialResponse,
    pub restart_required: bool,
}

/// Admin 凭据删除响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminCredentialDeleteResponse {
    pub credential_id: String,
    pub state_version: i64,
    pub restart_required: bool,
}
