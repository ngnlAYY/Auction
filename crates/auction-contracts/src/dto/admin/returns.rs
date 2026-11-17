//! 结算退件案件的 Admin DTO 与闭集取值。
//!
//! 案件以退件入站邮件为身份：`return_id` 就是该入站邮件 ID。列表与详情只给安全摘要，不含正文、
//! 发件人原文、Cookie 或装备 key；资产、金额与收件人只来自服务端冻结事实，命令请求只携带
//! `command_id`、案件版本与动作名。闭集取值与服务端领域枚举逐项一致（`auction-http` 的测试
//! 核对），浏览器按闭集失败关闭解码。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::super::response::deserialize_required_option;
use super::pagination::AdminStrictPaginationResponse;

/// 列表单页上限，与应用层端口一致。
pub const ADMIN_SETTLEMENT_RETURN_MAX_PAGE_SIZE: i64 = 200;

/// 列表行偏移上限，与应用层端口一致；超过时整页请求拒绝。
pub const ADMIN_SETTLEMENT_RETURN_MAX_OFFSET: i64 = 10_000;

/// 案件状态闭集（`settlement_return_cases.status`）。
pub const ADMIN_SETTLEMENT_RETURN_STATUSES: [&str; 10] = [
    "quarantined",
    "recheck_pending",
    "awaiting_decision",
    "take_pending",
    "take_unknown",
    "resend_pending",
    "held",
    "resent",
    "inbox_closed",
    "reviewed",
];

/// 已结案状态：代管、已补寄、留箱与已核对；其余状态计入待办。
pub const ADMIN_SETTLEMENT_RETURN_CLOSED_STATUSES: [&str; 4] =
    ["held", "resent", "inbox_closed", "reviewed"];

/// 证据分类闭集（`settlement_return_evidence.match_state`）。
pub const ADMIN_SETTLEMENT_RETURN_MATCH_STATES: [&str; 4] =
    ["untrusted", "unbound", "fulfillment_terminal", "manual_reconcile"];

/// 安全原因码闭集（`settlement_return_cases.reason_code`）。
pub const ADMIN_SETTLEMENT_RETURN_REASONS: [&str; 14] = [
    "source_unproven",
    "evidence_invalid",
    "obligation_unknown",
    "subject_kind_mismatch",
    "delivery_unsent",
    "world_mismatch",
    "dispatch_unknown",
    "dispatch_ambiguous",
    "dispatch_already_returned",
    "dispatch_superseded",
    "body_unproven",
    "returned_assets_match",
    "returned_assets_mismatch",
    "received_before_send",
];

/// 动作闭集；`allowed_actions`、阻断原因与命令结果共用。
pub const ADMIN_SETTLEMENT_RETURN_ACTIONS: [&str; 6] = [
    "reclaim_and_resend",
    "reclaim_and_hold",
    "resend_from_custody",
    "close_inbox",
    "acknowledge",
    "recheck",
];

/// `POST …/handle` 接受的动作：除重新核验外的全部动作；重新核验只走 `POST …/recheck`。
pub const ADMIN_SETTLEMENT_RETURN_HANDLE_ACTIONS: [&str; 5] =
    ["reclaim_and_resend", "reclaim_and_hold", "resend_from_custody", "close_inbox", "acknowledge"];

/// 动作被拒绝的类型化原因闭集。
pub const ADMIN_SETTLEMENT_RETURN_BLOCKERS: [&str; 8] = [
    "untrusted",
    "unbound",
    "assets_mismatch",
    "state_conflict",
    "resend_limit",
    "obligation_ineligible",
    "proof_required",
    "send_unresolved",
];

/// 资产当前位置闭集；与资产对账使用同一组名称，互斥且不重复计数。
pub const ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS: [&str; 7] = [
    "unverified",
    "returned_in_inbox",
    "take_in_progress",
    "location_unknown",
    "taken_awaiting_resend",
    "taken_in_custody",
    "resent_in_transit",
];

/// 领回后的处理计划闭集。
pub const ADMIN_SETTLEMENT_RETURN_PLANS: [&str; 2] = ["resend", "hold"];

/// 案件事件闭集（`settlement_return_events.event_kind`）。
pub const ADMIN_SETTLEMENT_RETURN_EVENT_KINDS: [&str; 13] = [
    "opened",
    "acknowledged",
    "recheck_requested",
    "recheck_rebound",
    "recheck_unchanged",
    "take_requested",
    "take_activated",
    "take_proven",
    "take_unknown",
    "held",
    "resend_authorized",
    "resent",
    "inbox_closed",
];

/// 持久 Take 的执行状态闭集（`settlement_return_actions.execution_state`）。
pub const ADMIN_SETTLEMENT_RETURN_TAKE_STATES: [&str; 4] =
    ["pending", "enqueued", "satisfied", "manual_reconcile"];

/// 结算义务种类闭集（列表筛选与摘要）。
pub const ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS: [&str; 7] = [
    "winner_equipment_cod",
    "winner_payment_carrier_cod",
    "winner_equipment_handoff",
    "seller_payout",
    "buyer_refund",
    "unsold_return",
    "withdrawn_return",
];

/// 案件的安全摘要；列表行与详情共用。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnSummaryResponse {
    /// 案件 ID，即退件入站邮件 ID。
    pub return_id: String,
    pub case_version: i64,
    pub status: String,
    pub match_state: String,
    pub reason_code: String,
    pub world_id: String,
    /// 退件在 Bot 收件箱中的远端 MID。
    pub external_mail_id: String,
    pub received_at: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub obligation_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub obligation_kind: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub auction_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub lot_id: Option<String>,
    /// 被退回的原派发投递；只在已绑定时存在。
    #[serde(deserialize_with = "deserialize_required_option")]
    pub delivery_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub dispatch_no: Option<u32>,
    /// 原派发冻结收件人；补寄只寄给这个名字。
    #[serde(deserialize_with = "deserialize_required_option")]
    pub recipient: Option<String>,
    pub credits: i64,
    pub cod: i64,
    pub attachment_count: u32,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub plan: Option<String>,
    /// 案件建立时冻结的补寄上限（不含首寄）。
    pub resend_limit: u8,
    /// 原义务已发生的补寄次数；未绑定义务时为空。
    #[serde(deserialize_with = "deserialize_required_option")]
    pub resends_used: Option<u32>,
    pub asset_location: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub closed_at: Option<String>,
}

/// `GET /api/v1/admin/settlement-returns` 的分页结果。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnListResponse {
    pub returns: Vec<AdminSettlementReturnSummaryResponse>,
    pub pagination: AdminStrictPaginationResponse,
}

/// 案件处理事件（只追加）。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnEventResponse {
    pub case_version: i64,
    pub event_kind: String,
    pub occurred_at: String,
    /// Admin 命令触发时为操作者；Worker 事件为空。
    #[serde(deserialize_with = "deserialize_required_option")]
    pub actor_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub command_id: Option<String>,
    /// 安全细节对象（分类、原因码、派发编号、投递 ID、失败码等），不含正文或秘密。
    pub details: Value,
}

/// 原义务的一次派发。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnDispatchResponse {
    pub dispatch_no: u32,
    pub delivery_id: String,
    /// 投递当前状态；补寄已授权但尚未准备时为空。
    #[serde(deserialize_with = "deserialize_required_option")]
    pub delivery_status: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub sending_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub completed_at: Option<String>,
    /// 授权这次补寄的退件案件；首寄为空。
    #[serde(deserialize_with = "deserialize_required_option")]
    pub authorized_by: Option<String>,
}

/// 本案的持久 Take。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnTakeResponse {
    pub execution_state: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub delivery_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub delivery_status: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub failure_code: Option<String>,
    /// 精确领回证明成立的时刻。
    #[serde(deserialize_with = "deserialize_required_option")]
    pub proven_at: Option<String>,
}

/// 代管责任：原权利人与代管资产；权益保留，不归 Bot 自有。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnCustodyResponse {
    /// 原派发冻结收件人（原权利人）。
    pub beneficiary: String,
    pub credits: i64,
    pub equipment_ids: Vec<String>,
}

/// 一个不被允许的动作及其类型化原因。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnBlockerResponse {
    pub action: String,
    pub blocker: String,
}

/// `GET /api/v1/admin/settlement-returns/{return_id}` 的详情；`allowed_actions` 与 `blockers`
/// 合起来恰好覆盖动作闭集，服务端在命令事务内仍会锁内复核。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnDetailResponse {
    pub summary: AdminSettlementReturnSummaryResponse,
    pub events: Vec<AdminSettlementReturnEventResponse>,
    pub dispatches: Vec<AdminSettlementReturnDispatchResponse>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub take: Option<AdminSettlementReturnTakeResponse>,
    /// 本案授权的补寄派发编号。
    #[serde(deserialize_with = "deserialize_required_option")]
    pub resend_dispatch_no: Option<u32>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub custody: Option<AdminSettlementReturnCustodyResponse>,
    pub allowed_actions: Vec<String>,
    pub blockers: Vec<AdminSettlementReturnBlockerResponse>,
    /// 案件待重新核验（`recheck_pending`），但没有在线 Worker 能只读读取该世界的 MoogleMail：
    /// 核验能力不可用，案件保持待核验，能力恢复后自动继续。其他状态恒为 `false`。
    pub recheck_unavailable: bool,
    /// 案件待领回（`take_pending`）、Take 尚未执行，但没有 Worker 能以持久凭据身份执行该世界的
    /// Take（只配置 Cookie 或未启用邮件）：领回能力不可用，案件停在待领回，能力恢复后自动继续。
    /// 其他状态恒为 `false`。
    pub take_unavailable: bool,
}

/// `POST …/{return_id}/handle` 请求；动作只取 [`ADMIN_SETTLEMENT_RETURN_HANDLE_ACTIONS`]。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnHandleRequest {
    pub command_id: Uuid,
    /// 页面读取时的案件版本。
    pub expected_version: i64,
    pub action: String,
}

/// `POST …/{return_id}/recheck` 请求。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnRecheckRequest {
    pub command_id: Uuid,
    pub expected_version: i64,
}

/// 命令结果；同一 `command_id` 原样重试返回首次提交时的结果。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnCommandResponse {
    pub command_id: String,
    pub return_id: String,
    pub action: String,
    /// 命令提交后的案件版本。
    pub case_version: i64,
    pub status: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        ADMIN_SETTLEMENT_RETURN_ACTIONS, ADMIN_SETTLEMENT_RETURN_CLOSED_STATUSES,
        ADMIN_SETTLEMENT_RETURN_HANDLE_ACTIONS, ADMIN_SETTLEMENT_RETURN_STATUSES,
        AdminSettlementReturnHandleRequest, AdminSettlementReturnSummaryResponse,
    };

    fn summary() -> serde_json::Value {
        json!({
            "return_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
            "case_version": 1,
            "status": "quarantined",
            "match_state": "unbound",
            "reason_code": "dispatch_unknown",
            "world_id": "main",
            "external_mail_id": "12345",
            "received_at": "2026-11-16T00:00:00Z",
            "obligation_id": null,
            "obligation_kind": null,
            "auction_id": null,
            "lot_id": null,
            "delivery_id": null,
            "dispatch_no": null,
            "recipient": null,
            "credits": 0,
            "cod": 0,
            "attachment_count": 0,
            "plan": null,
            "resend_limit": 3,
            "resends_used": null,
            "asset_location": "unverified",
            "created_at": "2026-11-16T00:00:00Z",
            "updated_at": "2026-11-16T00:00:00Z",
            "closed_at": null
        })
    }

    /// 可空字段必须显式出现（可为 null）；未知字段与缺字段都失败关闭。
    #[test]
    fn summary_requires_every_key_and_rejects_unknown_fields() {
        assert!(serde_json::from_value::<AdminSettlementReturnSummaryResponse>(summary()).is_ok());
        let mut missing = summary();
        if let Some(object) = missing.as_object_mut() {
            object.remove("closed_at");
        }
        assert!(serde_json::from_value::<AdminSettlementReturnSummaryResponse>(missing).is_err());
        let mut extra = summary();
        if let Some(object) = extra.as_object_mut() {
            object.insert("body".to_owned(), json!("x"));
        }
        assert!(serde_json::from_value::<AdminSettlementReturnSummaryResponse>(extra).is_err());
    }

    /// handle 请求只接受三个字段；动作闭集排除重新核验，已结案状态是状态闭集的子集。
    #[test]
    fn handle_request_and_closed_sets_are_consistent() {
        let request = json!({
            "command_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
            "expected_version": 2,
            "action": "close_inbox"
        });
        assert!(serde_json::from_value::<AdminSettlementReturnHandleRequest>(request).is_ok());
        let extra = json!({
            "command_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
            "expected_version": 2,
            "action": "close_inbox",
            "recipient": "Other"
        });
        assert!(serde_json::from_value::<AdminSettlementReturnHandleRequest>(extra).is_err());
        assert!(!ADMIN_SETTLEMENT_RETURN_HANDLE_ACTIONS.contains(&"recheck"));
        assert!(
            ADMIN_SETTLEMENT_RETURN_HANDLE_ACTIONS
                .iter()
                .all(|action| ADMIN_SETTLEMENT_RETURN_ACTIONS.contains(action))
        );
        assert!(
            ADMIN_SETTLEMENT_RETURN_CLOSED_STATUSES
                .iter()
                .all(|status| ADMIN_SETTLEMENT_RETURN_STATUSES.contains(status))
        );
    }
}
