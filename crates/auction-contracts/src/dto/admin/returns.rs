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
pub const ADMIN_SETTLEMENT_RETURN_ACTIONS: [&str; 7] = [
    "reclaim_and_resend",
    "reclaim_and_hold",
    "resend_from_custody",
    "close_inbox",
    "acknowledge",
    "recheck",
    "retry_take",
];

/// `POST …/handle` 接受的动作：除重新核验外的全部动作；重新核验只走 `POST …/recheck`。
pub const ADMIN_SETTLEMENT_RETURN_HANDLE_ACTIONS: [&str; 6] = [
    "reclaim_and_resend",
    "reclaim_and_hold",
    "resend_from_custody",
    "close_inbox",
    "acknowledge",
    "retry_take",
];

/// 动作被拒绝的类型化原因闭集。
pub const ADMIN_SETTLEMENT_RETURN_BLOCKERS: [&str; 9] = [
    "untrusted",
    "unbound",
    "assets_mismatch",
    "state_conflict",
    "resend_limit",
    "obligation_ineligible",
    "proof_required",
    "send_unresolved",
    "observation_required",
];

/// Take 结果未知的原因闭集（详情 `take_unknown_reason`）：前五项是邮箱维护在原 Take 会话里的
/// 观察，后四项由持久事实推出；只有 `assets_claimable` 允许再次领回。
pub const ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS: [&str; 9] = [
    "assets_claimable",
    "assets_unclaimable",
    "assets_changed",
    "mail_missing",
    "receipt_unproven",
    "session_changed",
    "take_unverifiable",
    "disposition_conflict",
    "awaiting_observation",
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

fn deserialize_asset_location<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if !ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS.contains(&value.as_str()) {
        return Err(serde::de::Error::custom("资产位置不在闭集内"));
    }
    Ok(value)
}

/// 领回后的处理计划闭集。
pub const ADMIN_SETTLEMENT_RETURN_PLANS: [&str; 2] = ["resend", "hold"];

/// 案件事件闭集（`settlement_return_events.event_kind`）。
pub const ADMIN_SETTLEMENT_RETURN_EVENT_KINDS: [&str; 15] = [
    "opened",
    "acknowledged",
    "recheck_requested",
    "recheck_rebound",
    "recheck_unchanged",
    "take_requested",
    "take_activated",
    "take_proven",
    "take_unknown",
    "take_observed",
    "take_retry_requested",
    "held",
    "resend_authorized",
    "resent",
    "inbox_closed",
];
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminSettlementReturnEventKind {
    Opened,
    Acknowledged,
    RecheckRequested,
    RecheckRebound,
    RecheckUnchanged,
    TakeRequested,
    TakeActivated,
    TakeProven,
    TakeUnknown,
    TakeObserved,
    TakeRetryRequested,
    Held,
    ResendAuthorized,
    Resent,
    InboxClosed,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminSettlementReturnStatus {
    Quarantined,
    RecheckPending,
    AwaitingDecision,
    TakePending,
    TakeUnknown,
    ResendPending,
    Held,
    Resent,
    InboxClosed,
    Reviewed,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminSettlementReturnMatchState {
    Untrusted,
    Unbound,
    FulfillmentTerminal,
    ManualReconcile,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminSettlementReturnReason {
    SourceUnproven,
    EvidenceInvalid,
    ObligationUnknown,
    SubjectKindMismatch,
    DeliveryUnsent,
    WorldMismatch,
    DispatchUnknown,
    DispatchAmbiguous,
    DispatchAlreadyReturned,
    DispatchSuperseded,
    BodyUnproven,
    ReturnedAssetsMatch,
    ReturnedAssetsMismatch,
    ReceivedBeforeSend,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminSettlementReturnAction {
    ReclaimAndResend,
    ReclaimAndHold,
    ResendFromCustody,
    CloseInbox,
    Acknowledge,
    Recheck,
    RetryTake,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminSettlementReturnHandleAction {
    ReclaimAndResend,
    ReclaimAndHold,
    ResendFromCustody,
    CloseInbox,
    Acknowledge,
    RetryTake,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminSettlementReturnBlocker {
    Untrusted,
    Unbound,
    AssetsMismatch,
    StateConflict,
    ResendLimit,
    ObligationIneligible,
    ProofRequired,
    SendUnresolved,
    ObservationRequired,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminSettlementReturnPlan {
    Resend,
    Hold,
}

/// 案件摘要。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnSummaryResponse {
    pub return_id: String,
    pub case_version: i64,
    pub status: AdminSettlementReturnStatus,
    pub match_state: AdminSettlementReturnMatchState,
    pub reason_code: AdminSettlementReturnReason,
    pub world_id: String,
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
    #[serde(deserialize_with = "deserialize_required_option")]
    pub delivery_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub dispatch_no: Option<u32>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub recipient: Option<String>,
    pub credits: i64,
    pub cod: i64,
    pub attachment_count: u32,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub plan: Option<AdminSettlementReturnPlan>,
    pub resend_limit: u8,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub resends_used: Option<u32>,
    #[serde(deserialize_with = "deserialize_asset_location")]
    pub asset_location: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub closed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnListResponse {
    pub returns: Vec<AdminSettlementReturnSummaryResponse>,
    pub pagination: AdminStrictPaginationResponse,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnEventResponse {
    pub case_version: i64,
    pub event_kind: AdminSettlementReturnEventKind,
    pub occurred_at: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub actor_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub command_id: Option<String>,
    pub details: Value,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnDispatchResponse {
    pub dispatch_no: u32,
    pub delivery_id: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub delivery_status: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub sending_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub completed_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub authorized_by: Option<String>,
}

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
    #[serde(deserialize_with = "deserialize_required_option")]
    pub proven_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnCustodyResponse {
    pub beneficiary: String,
    pub credits: i64,
    pub equipment_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnBlockerResponse {
    pub action: AdminSettlementReturnAction,
    pub blocker: AdminSettlementReturnBlocker,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnDetailResponse {
    pub summary: AdminSettlementReturnSummaryResponse,
    pub events: Vec<AdminSettlementReturnEventResponse>,
    pub dispatches: Vec<AdminSettlementReturnDispatchResponse>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub take: Option<AdminSettlementReturnTakeResponse>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub resend_dispatch_no: Option<u32>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub custody: Option<AdminSettlementReturnCustodyResponse>,
    pub allowed_actions: Vec<AdminSettlementReturnAction>,
    pub blockers: Vec<AdminSettlementReturnBlockerResponse>,
    pub recheck_unavailable: bool,
    pub take_unavailable: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub take_unknown_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnHandleRequest {
    pub command_id: Uuid,
    pub expected_version: i64,
    pub action: AdminSettlementReturnHandleAction,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnRecheckRequest {
    pub command_id: Uuid,
    pub expected_version: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementReturnCommandResponse {
    pub command_id: String,
    pub return_id: String,
    pub action: AdminSettlementReturnAction,
    pub case_version: i64,
    pub status: AdminSettlementReturnStatus,
}

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

    #[test]
    fn event_kinds_and_handle_actions_are_closed_with_legacy_held() {
        assert_eq!(
            serde_json::from_str::<super::AdminSettlementReturnEventKind>("\"held\"").unwrap(),
            super::AdminSettlementReturnEventKind::Held
        );
        assert!(
            serde_json::from_str::<super::AdminSettlementReturnEventKind>("\"future_kind\"")
                .is_err()
        );
        let mut unknown = json!({
            "command_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
            "expected_version": 2,
            "action": "recheck"
        });
        assert!(
            serde_json::from_value::<AdminSettlementReturnHandleRequest>(unknown.take()).is_err()
        );
    }
}
