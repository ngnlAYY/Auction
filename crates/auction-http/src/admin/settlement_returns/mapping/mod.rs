//! 结算退件应用读模型到 Admin DTO 的映射与安全投影校验。
//!
//! 只输出闭集名称、标识、金额与时间；读模型自相矛盾（分页越界、事件版本乱序、动作集合不完整
//! 等）时返回 `None`，由 handler 按内部错误失败关闭，不把损坏数据交给浏览器。

use std::collections::HashSet;

use auction_application::settlement::{
    SettlementReturnAssetLocation, SettlementReturnCaseSummary, SettlementReturnCommandResult,
    SettlementReturnCustody, SettlementReturnDetail, SettlementReturnDispatch,
    SettlementReturnMatch, SettlementReturnPage, SettlementReturnReason, SettlementReturnTake,
};
use auction_contracts::{
    AdminSettlementReturnAction, AdminSettlementReturnBlocker,
    AdminSettlementReturnBlockerResponse, AdminSettlementReturnCommandResponse,
    AdminSettlementReturnCustodyResponse, AdminSettlementReturnDetailResponse,
    AdminSettlementReturnDispatchResponse, AdminSettlementReturnListResponse,
    AdminSettlementReturnMatchState, AdminSettlementReturnPlan, AdminSettlementReturnReason,
    AdminSettlementReturnStatus, AdminSettlementReturnSummaryResponse,
    AdminSettlementReturnTakeResponse, AdminStrictPaginationResponse,
};
use auction_domain::{
    SettlementReturnAction, SettlementReturnBlocker, SettlementReturnCaseStatus,
    SettlementReturnPlan, valid_text,
};
use uuid::Uuid;

use crate::mapping::timestamp;

mod events;
mod validation;

pub(super) use events::map_event;

/// 映射一页列表；`page`、`limit` 与 `offset` 是本次请求的回显与校验依据。
pub(super) fn map_page(
    result: SettlementReturnPage,
    page: i64,
    limit: i64,
    offset: i64,
) -> Option<AdminSettlementReturnListResponse> {
    let count = i64::try_from(result.items.len()).ok()?;
    // 总数与当前页读自同一快照：非空页的末行不可能越过总数。
    if page < 1
        || limit < 1
        || offset < 0
        || page.checked_sub(1)?.checked_mul(limit)? != offset
        || count > limit
        || result.total < 0
        || (count > 0 && result.total < offset.checked_add(count)?)
    {
        return None;
    }
    let returns = result.items.into_iter().map(map_summary).collect::<Option<Vec<_>>>()?;
    Some(AdminSettlementReturnListResponse {
        returns,
        pagination: AdminStrictPaginationResponse { page, page_size: limit, total: result.total },
    })
}

/// 映射案件摘要；版本、世界或金额不合法时拒绝。
pub(super) fn map_summary(
    summary: SettlementReturnCaseSummary,
) -> Option<AdminSettlementReturnSummaryResponse> {
    if summary.case_version < 1
        || !valid_summary_binding(&summary)
        || !matches!(summary.world_id.as_str(), "main" | "isekai")
        || summary.credits < 0
        || summary.cod < 0
        || summary.external_mail_id.is_empty()
    {
        return None;
    }
    Some(AdminSettlementReturnSummaryResponse {
        return_id: summary.return_mail_id.to_string(),
        case_version: summary.case_version,
        status: admin_status(summary.status),
        match_state: admin_match_state(summary.match_state),
        reason_code: admin_reason(summary.reason),
        world_id: summary.world_id,
        external_mail_id: summary.external_mail_id,
        received_at: timestamp(summary.received_at),
        obligation_id: summary.obligation_id.map(|value| value.to_string()),
        obligation_kind: summary.obligation_kind.map(|kind| kind.as_str().to_owned()),
        auction_id: summary.auction_id.map(|value| value.to_string()),
        lot_id: summary.lot_id.map(|value| value.to_string()),
        delivery_id: summary.delivery_id.map(|value| value.to_string()),
        dispatch_no: summary.dispatch_no,
        recipient: summary.recipient,
        credits: summary.credits,
        cod: summary.cod,
        attachment_count: summary.attachment_count,
        plan: summary.plan.map(admin_plan),
        resend_limit: summary.resend_limit,
        resends_used: summary.resends_used,
        asset_location: summary.asset_location.as_str().to_owned(),
        created_at: timestamp(summary.created_at),
        updated_at: timestamp(summary.updated_at),
        closed_at: summary.closed_at.map(timestamp),
    })
}
fn valid_summary_binding(summary: &SettlementReturnCaseSummary) -> bool {
    let bound = summary.obligation_id.is_some();
    let has_dispatch = summary.delivery_id.is_some();
    bound == summary.obligation_kind.is_some()
        && bound == summary.auction_id.is_some()
        && bound == summary.lot_id.is_some()
        && has_dispatch == summary.dispatch_no.is_some()
        && has_dispatch == summary.recipient.is_some()
        && summary.obligation_id.is_none_or(|id| !id.is_nil())
        && summary.auction_id.is_none_or(|id| !id.is_nil())
        && summary.lot_id.is_none_or(|id| !id.is_nil())
        && summary.delivery_id.is_none_or(|id| !id.is_nil())
        && summary.dispatch_no.is_none_or(|dispatch_no| dispatch_no > 0)
        && summary.recipient.as_deref().is_none_or(|value| valid_text(value, 256))
        && summary.asset_location == SettlementReturnAssetLocation::of(summary.status, has_dispatch)
}

/// 映射详情；摘要必须正是请求的案件，事件版本严格递增，派发编号严格递增，允许动作与阻断
/// 原因合起来恰好覆盖一次动作闭集。
pub(super) fn map_detail(
    detail: SettlementReturnDetail,
    requested: Uuid,
) -> Option<AdminSettlementReturnDetailResponse> {
    if detail.summary.return_mail_id != requested
        || !strictly_increasing(detail.events.iter().map(|event| event.case_version))
        || detail.events.iter().any(|event| event.case_version < 1)
        || !valid_dispatch_projection(&detail)
        || detail.take.as_ref().is_some_and(|take| !valid_take(take))
        // 核验能力不可用只说明待重新核验的案件为何不前进，其他状态出现即投影不一致。
        || (detail.recheck_unavailable
            && detail.summary.status != SettlementReturnCaseStatus::RecheckPending)
        // 领回能力不可用同理只说明待领回的案件为何不前进。
        || (detail.take_unavailable
            && detail.summary.status != SettlementReturnCaseStatus::TakePending)
        || detail.custody.is_some() != (detail.summary.status == SettlementReturnCaseStatus::Held)
        || detail.custody.as_ref().is_some_and(|custody| !valid_custody(custody))
        // 结果未知的案件必有原因，其他状态没有。
        || detail.take_unknown_reason.is_some()
            != (detail.summary.status == SettlementReturnCaseStatus::TakeUnknown)
    {
        return None;
    }
    let mut seen = HashSet::with_capacity(SettlementReturnAction::ALL.len());
    let complete = detail
        .allowed_actions
        .iter()
        .chain(detail.blockers.iter().map(|(action, _)| action))
        .all(|action| seen.insert(*action))
        && seen.len() == SettlementReturnAction::ALL.len();
    if !complete {
        return None;
    }
    let events = detail.events.into_iter().map(map_event).collect::<Option<Vec<_>>>()?;

    Some(AdminSettlementReturnDetailResponse {
        summary: map_summary(detail.summary)?,
        events,
        dispatches: detail.dispatches.into_iter().map(map_dispatch).collect(),
        take: detail.take.map(map_take),
        resend_dispatch_no: detail.resend_dispatch_no,
        custody: detail.custody.map(map_custody),
        allowed_actions: detail.allowed_actions.into_iter().map(admin_action).collect(),
        blockers: detail
            .blockers
            .into_iter()
            .map(|(action, blocker)| AdminSettlementReturnBlockerResponse {
                action: admin_action(action),
                blocker: admin_blocker(blocker),
            })
            .collect(),
        recheck_unavailable: detail.recheck_unavailable,
        take_unavailable: detail.take_unavailable,
        take_unknown_reason: detail.take_unknown_reason.map(|reason| reason.as_str().to_owned()),
    })
}

/// 映射命令结果。
pub(super) fn map_command_result(
    result: SettlementReturnCommandResult,
) -> AdminSettlementReturnCommandResponse {
    AdminSettlementReturnCommandResponse {
        command_id: result.command_id.as_uuid().to_string(),
        return_id: result.return_mail_id.to_string(),
        action: admin_action(result.action),
        case_version: result.case_version,
        status: admin_status(result.status),
    }
}

fn strictly_increasing(mut values: impl Iterator<Item = i64>) -> bool {
    let mut previous = None;
    values.all(|value| {
        let ordered = previous.is_none_or(|previous| value > previous);
        previous = Some(value);
        ordered
    })
}

fn admin_status(value: SettlementReturnCaseStatus) -> AdminSettlementReturnStatus {
    use auction_contracts::AdminSettlementReturnStatus as Dto;
    use auction_domain::SettlementReturnCaseStatus as Source;

    match value {
        Source::Quarantined => Dto::Quarantined,
        Source::RecheckPending => Dto::RecheckPending,
        Source::AwaitingDecision => Dto::AwaitingDecision,
        Source::TakePending => Dto::TakePending,
        Source::TakeUnknown => Dto::TakeUnknown,
        Source::ResendPending => Dto::ResendPending,
        Source::Held => Dto::Held,
        Source::Resent => Dto::Resent,
        Source::InboxClosed => Dto::InboxClosed,
        Source::Reviewed => Dto::Reviewed,
    }
}

fn admin_match_state(value: SettlementReturnMatch) -> AdminSettlementReturnMatchState {
    use auction_application::settlement::SettlementReturnMatch as Source;
    use auction_contracts::AdminSettlementReturnMatchState as Dto;

    match value {
        Source::Untrusted => Dto::Untrusted,
        Source::Unbound => Dto::Unbound,
        Source::FulfillmentTerminal => Dto::FulfillmentTerminal,
        Source::ManualReconcile => Dto::ManualReconcile,
    }
}

fn admin_reason(value: SettlementReturnReason) -> AdminSettlementReturnReason {
    use auction_application::settlement::SettlementReturnReason as Source;
    use auction_contracts::AdminSettlementReturnReason as Dto;

    match value {
        Source::SourceUnproven => Dto::SourceUnproven,
        Source::EvidenceInvalid => Dto::EvidenceInvalid,
        Source::ObligationUnknown => Dto::ObligationUnknown,
        Source::SubjectKindMismatch => Dto::SubjectKindMismatch,
        Source::DeliveryUnsent => Dto::DeliveryUnsent,
        Source::WorldMismatch => Dto::WorldMismatch,
        Source::DispatchUnknown => Dto::DispatchUnknown,
        Source::DispatchAmbiguous => Dto::DispatchAmbiguous,
        Source::DispatchAlreadyReturned => Dto::DispatchAlreadyReturned,
        Source::DispatchSuperseded => Dto::DispatchSuperseded,
        Source::BodyUnproven => Dto::BodyUnproven,
        Source::ReturnedAssetsMatch => Dto::ReturnedAssetsMatch,
        Source::ReturnedAssetsMismatch => Dto::ReturnedAssetsMismatch,
        Source::ReceivedBeforeSend => Dto::ReceivedBeforeSend,
    }
}

fn admin_action(value: SettlementReturnAction) -> AdminSettlementReturnAction {
    use auction_contracts::AdminSettlementReturnAction as Dto;
    use auction_domain::SettlementReturnAction as Source;

    match value {
        Source::ReclaimAndResend => Dto::ReclaimAndResend,
        Source::ReclaimAndHold => Dto::ReclaimAndHold,
        Source::ResendFromCustody => Dto::ResendFromCustody,
        Source::CloseInbox => Dto::CloseInbox,
        Source::Acknowledge => Dto::Acknowledge,
        Source::Recheck => Dto::Recheck,
        Source::RetryTake => Dto::RetryTake,
    }
}

fn admin_blocker(value: SettlementReturnBlocker) -> AdminSettlementReturnBlocker {
    use auction_contracts::AdminSettlementReturnBlocker as Dto;
    use auction_domain::SettlementReturnBlocker as Source;

    match value {
        Source::Untrusted => Dto::Untrusted,
        Source::Unbound => Dto::Unbound,
        Source::AssetsMismatch => Dto::AssetsMismatch,
        Source::StateConflict => Dto::StateConflict,
        Source::ResendLimit => Dto::ResendLimit,
        Source::ObligationIneligible => Dto::ObligationIneligible,
        Source::ProofRequired => Dto::ProofRequired,
        Source::SendUnresolved => Dto::SendUnresolved,
        Source::ObservationRequired => Dto::ObservationRequired,
    }
}

fn admin_plan(value: SettlementReturnPlan) -> AdminSettlementReturnPlan {
    use auction_contracts::AdminSettlementReturnPlan as Dto;
    use auction_domain::SettlementReturnPlan as Source;

    match value {
        Source::Resend => Dto::Resend,
        Source::Hold => Dto::Hold,
    }
}

fn valid_dispatch(dispatch: &SettlementReturnDispatch) -> bool {
    let authorized = match dispatch.dispatch_no {
        1 => dispatch.authorized_by.is_none(),
        _ => dispatch.authorized_by.is_some_and(|return_id| !return_id.is_nil()),
    };
    authorized
        && !dispatch.delivery_id.is_nil()
        && valid_delivery_timeline(
            dispatch.delivery_status.as_deref(),
            dispatch.sending_at,
            dispatch.completed_at,
        )
}

fn valid_delivery_timeline(
    status: Option<&str>,
    sending_at: Option<auction_time::Timestamp>,
    completed_at: Option<auction_time::Timestamp>,
) -> bool {
    if sending_at.is_some_and(|time| time.timestamp_micros() == 0)
        || completed_at.is_some_and(|time| time.timestamp_micros() == 0)
        || completed_at.zip(sending_at).is_some_and(|(completed, sending)| completed < sending)
    {
        return false;
    }
    match status {
        None => sending_at.is_none() && completed_at.is_none(),
        Some("prepared" | "pending" | "claimed") => sending_at.is_none() && completed_at.is_none(),
        Some("sending") => sending_at.is_some() && completed_at.is_none(),
        Some("succeeded") => sending_at.is_some() && completed_at.is_some(),
        Some("failed" | "manual_reconcile") => completed_at.is_some(),
        Some(_) => false,
    }
}

fn valid_take(take: &SettlementReturnTake) -> bool {
    let delivery_id = take.delivery_id.is_some_and(|id| !id.is_nil());
    let status_valid = take.delivery_status.as_deref().is_none_or(|status| {
        matches!(
            status,
            "prepared"
                | "pending"
                | "claimed"
                | "sending"
                | "succeeded"
                | "failed"
                | "manual_reconcile"
        )
    });
    let failure_valid = take.failure_code.as_deref().is_none_or(|code| {
        valid_text(code, 128)
            && !code.is_empty()
            && code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    });
    let proof_valid = take.proven_at.is_none_or(|time| time.timestamp_micros() != 0);
    status_valid
        && failure_valid
        && proof_valid
        && match take.execution_state.as_str() {
            "pending" => {
                take.delivery_id.is_none()
                    && take.delivery_status.is_none()
                    && take.failure_code.is_none()
                    && take.proven_at.is_none()
            }
            "enqueued" => {
                // 投递与案件由不同事务推进；终态投递可以等待 Worker 收口动作。
                delivery_id
                    && take.delivery_status.is_some()
                    && take.failure_code.is_none()
                    && take.proven_at.is_none()
            }
            "satisfied" => {
                delivery_id
                    && take.delivery_status.as_deref() == Some("succeeded")
                    && take.failure_code.is_none()
                    && take.proven_at.is_some()
            }
            "manual_reconcile" => {
                delivery_id
                    && take.delivery_status.as_deref().is_some_and(|status| {
                        matches!(status, "succeeded" | "failed" | "manual_reconcile")
                    })
                    && take.failure_code.is_some()
                    && take.proven_at.is_none()
            }
            _ => false,
        }
}
fn valid_custody(custody: &SettlementReturnCustody) -> bool {
    valid_text(&custody.beneficiary, 256)
        && custody.credits >= 0
        && custody.equipment_ids.iter().all(|id| {
            (1..=32).contains(&id.len())
                && id.bytes().all(|byte| byte.is_ascii_digit())
                && id.bytes().any(|byte| byte != b'0')
        })
        && custody.equipment_ids.iter().collect::<HashSet<_>>().len() == custody.equipment_ids.len()
}

fn map_dispatch(dispatch: SettlementReturnDispatch) -> AdminSettlementReturnDispatchResponse {
    AdminSettlementReturnDispatchResponse {
        dispatch_no: dispatch.dispatch_no,
        delivery_id: dispatch.delivery_id.to_string(),
        delivery_status: dispatch.delivery_status,
        sending_at: dispatch.sending_at.map(timestamp),
        completed_at: dispatch.completed_at.map(timestamp),
        authorized_by: dispatch.authorized_by.map(|value| value.to_string()),
    }
}

fn map_take(take: SettlementReturnTake) -> AdminSettlementReturnTakeResponse {
    AdminSettlementReturnTakeResponse {
        execution_state: take.execution_state,
        delivery_id: take.delivery_id.map(|value| value.to_string()),
        delivery_status: take.delivery_status,
        failure_code: take.failure_code,
        proven_at: take.proven_at.map(timestamp),
    }
}

fn map_custody(custody: SettlementReturnCustody) -> AdminSettlementReturnCustodyResponse {
    AdminSettlementReturnCustodyResponse {
        beneficiary: custody.beneficiary,
        credits: custody.credits,
        equipment_ids: custody.equipment_ids,
    }
}

fn valid_dispatch_projection(detail: &SettlementReturnDetail) -> bool {
    if detail.summary.obligation_id.is_none()
        && (!detail.dispatches.is_empty() || detail.resend_dispatch_no.is_some())
    {
        return false;
    }
    for (index, dispatch) in detail.dispatches.iter().enumerate() {
        let expected_no = u32::try_from(index).ok().and_then(|value| value.checked_add(1));
        if expected_no != Some(dispatch.dispatch_no) || !valid_dispatch(dispatch) {
            return false;
        }
    }
    if let Some(delivery_id) = detail.summary.delivery_id
        && !detail.summary.dispatch_no.is_some_and(|dispatch_no| {
            detail.dispatches.iter().any(|dispatch| {
                dispatch.dispatch_no == dispatch_no && dispatch.delivery_id == delivery_id
            })
        })
    {
        return false;
    }

    let latest_authorized = detail
        .dispatches
        .iter()
        .filter(|dispatch| dispatch.authorized_by == Some(detail.summary.return_mail_id))
        .map(|dispatch| dispatch.dispatch_no)
        .max();
    match (detail.resend_dispatch_no, latest_authorized) {
        (None, None) => true,
        (Some(expected), Some(authorized)) => expected == authorized,
        (Some(expected), None) => {
            let next = detail
                .dispatches
                .last()
                .map_or(Some(1), |dispatch| dispatch.dispatch_no.checked_add(1));
            expected > 1 && next == Some(expected)
        }
        (None, Some(_)) => false,
    }
}
#[cfg(test)]
mod tests {
    use auction_application::settlement::{
        SettlementReturnAssetLocation, SettlementReturnCaseSummary, SettlementReturnDetail,
        SettlementReturnDispatch, SettlementReturnEvent, SettlementReturnMatch,
        SettlementReturnPage, SettlementReturnReason, SettlementReturnTake,
    };
    use auction_contracts::{
        ADMIN_SETTLEMENT_RETURN_ACTIONS, ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS,
        ADMIN_SETTLEMENT_RETURN_BLOCKERS, ADMIN_SETTLEMENT_RETURN_MATCH_STATES,
        ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS, ADMIN_SETTLEMENT_RETURN_PLANS,
        ADMIN_SETTLEMENT_RETURN_REASONS, ADMIN_SETTLEMENT_RETURN_STATUSES,
        ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS,
    };
    use auction_domain::{
        SettlementObligationKind, SettlementReturnAction, SettlementReturnBlocker,
        SettlementReturnCaseStatus, SettlementReturnPlan, SettlementReturnTakeUnknownReason,
    };
    use auction_time::Timestamp;
    use serde_json::json;
    use uuid::Uuid;

    use super::{map_detail, map_event, map_page, map_summary};

    const CASE: Uuid = Uuid::from_u128(0x0f8f_ad5b_d9cb_469f_a165_7086_7728_950e);

    fn summary() -> SettlementReturnCaseSummary {
        let at = Timestamp::from_ymd_hms(2026, 11, 16, 1, 2, 3).expect("时间");
        SettlementReturnCaseSummary {
            return_mail_id: CASE,
            case_version: 2,
            status: SettlementReturnCaseStatus::AwaitingDecision,
            match_state: SettlementReturnMatch::ManualReconcile,
            reason: SettlementReturnReason::ReturnedAssetsMatch,
            world_id: "main".to_owned(),
            external_mail_id: "993021".to_owned(),
            received_at: at,
            obligation_id: Some(Uuid::from_u128(2)),
            obligation_kind: Some(SettlementObligationKind::SellerPayout),
            auction_id: Some(Uuid::from_u128(3)),
            lot_id: Some(Uuid::from_u128(4)),
            delivery_id: Some(Uuid::from_u128(5)),
            dispatch_no: Some(1),
            recipient: Some("Seller".to_owned()),
            credits: 12_000,
            cod: 0,
            attachment_count: 0,
            plan: None,
            resend_limit: 3,
            resends_used: Some(0),
            asset_location: SettlementReturnAssetLocation::ReturnedInInbox,
            created_at: at,
            updated_at: at,
            closed_at: None,
        }
    }

    fn detail() -> SettlementReturnDetail {
        let at = Timestamp::from_ymd_hms(2026, 11, 16, 1, 2, 3).expect("时间");
        SettlementReturnDetail {
            summary: summary(),
            events: vec![
                SettlementReturnEvent {
                    case_version: 1,
                    event_kind: "opened".to_owned(),
                    occurred_at: at,
                    actor_id: None,
                    command_id: None,
                    details: json!({
                        "match_state": "manual_reconcile",
                        "reason_code": "returned_assets_match",
                        "dispatch_no": 1,
                    }),
                },
                SettlementReturnEvent {
                    case_version: 2,
                    event_kind: "recheck_rebound".to_owned(),
                    occurred_at: at,
                    actor_id: None,
                    command_id: None,
                    details: json!({
                        "match_state": "manual_reconcile",
                        "reason_code": "returned_assets_match",
                        "delivery_id": Uuid::from_u128(5),
                        "dispatch_no": 1,
                        "verified_version": 1,
                    }),
                },
            ],
            dispatches: vec![SettlementReturnDispatch {
                dispatch_no: 1,
                delivery_id: Uuid::from_u128(5),
                delivery_status: Some("succeeded".to_owned()),
                sending_at: Some(at),
                completed_at: Some(at),
                authorized_by: None,
            }],
            take: None,
            resend_dispatch_no: None,
            custody: None,
            allowed_actions: vec![
                SettlementReturnAction::ReclaimAndResend,
                SettlementReturnAction::ReclaimAndHold,
                SettlementReturnAction::CloseInbox,
            ],
            blockers: vec![
                (SettlementReturnAction::ResendFromCustody, SettlementReturnBlocker::StateConflict),
                (SettlementReturnAction::Acknowledge, SettlementReturnBlocker::StateConflict),
                (SettlementReturnAction::Recheck, SettlementReturnBlocker::StateConflict),
                (SettlementReturnAction::RetryTake, SettlementReturnBlocker::StateConflict),
            ],
            recheck_unavailable: false,
            take_unavailable: false,
            take_unknown_reason: None,
        }
    }

    /// 契约闭集与领域/应用闭集逐项一致；任一侧增删取值都会让浏览器解码与服务端分叉。
    #[test]
    fn contract_closed_sets_match_domain_names() {
        let names =
            |values: &[&str]| values.iter().map(|value| (*value).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            SettlementReturnCaseStatus::ALL
                .iter()
                .map(|value| value.as_str().to_owned())
                .collect::<Vec<_>>(),
            names(&ADMIN_SETTLEMENT_RETURN_STATUSES)
        );
        assert_eq!(
            SettlementReturnAction::ALL
                .iter()
                .map(|value| value.as_str().to_owned())
                .collect::<Vec<_>>(),
            names(&ADMIN_SETTLEMENT_RETURN_ACTIONS)
        );
        assert_eq!(
            SettlementReturnBlocker::ALL
                .iter()
                .map(|value| value.as_str().to_owned())
                .collect::<Vec<_>>(),
            names(&ADMIN_SETTLEMENT_RETURN_BLOCKERS)
        );
        assert_eq!(
            SettlementReturnPlan::ALL
                .iter()
                .map(|value| value.as_str().to_owned())
                .collect::<Vec<_>>(),
            names(&ADMIN_SETTLEMENT_RETURN_PLANS)
        );
        assert_eq!(
            SettlementReturnReason::ALL
                .iter()
                .map(|value| value.as_str().to_owned())
                .collect::<Vec<_>>(),
            names(&ADMIN_SETTLEMENT_RETURN_REASONS)
        );
        assert_eq!(
            SettlementReturnTakeUnknownReason::ALL
                .iter()
                .map(|value| value.as_str().to_owned())
                .collect::<Vec<_>>(),
            names(&ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS)
        );
        for name in ADMIN_SETTLEMENT_RETURN_MATCH_STATES {
            assert_eq!(
                SettlementReturnMatch::from_name(name).map(SettlementReturnMatch::as_str),
                Some(name)
            );
        }
        for name in ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS {
            assert_eq!(
                SettlementObligationKind::from_name(name).map(SettlementObligationKind::as_str),
                Some(name)
            );
        }
        for status in SettlementReturnCaseStatus::ALL {
            for bound in [false, true] {
                let location = SettlementReturnAssetLocation::of(*status, bound).as_str();
                assert!(ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS.contains(&location), "{location}");
            }
        }
    }

    /// 摘要映射只输出闭集名称、规范 UUID 与 RFC 3339 时间；非法世界或负金额失败关闭。
    #[test]
    fn summary_maps_closed_names_and_rejects_corrupt_values() {
        let mapped = map_summary(summary()).expect("合法摘要");
        assert_eq!(mapped.return_id, CASE.to_string());
        assert_eq!(mapped.status, auction_contracts::AdminSettlementReturnStatus::AwaitingDecision);
        assert_eq!(
            mapped.match_state,
            auction_contracts::AdminSettlementReturnMatchState::ManualReconcile
        );
        assert_eq!(
            mapped.reason_code,
            auction_contracts::AdminSettlementReturnReason::ReturnedAssetsMatch
        );
        assert_eq!(mapped.obligation_kind.as_deref(), Some("seller_payout"));
        assert_eq!(mapped.asset_location, "returned_in_inbox");
        assert_eq!(mapped.received_at, "2026-11-16T01:02:03.000000Z");
        let mut world = summary();
        world.world_id = "other".to_owned();
        assert!(map_summary(world).is_none());
        let mut credits = summary();
        credits.credits = -1;
        assert!(map_summary(credits).is_none());
        let mut version = summary();
        version.case_version = 0;
        assert!(map_summary(version).is_none());
    }

    /// 分页：行数不超过页长，非空页末行不越过总数。
    #[test]
    fn page_rejects_rows_beyond_limit_or_total() {
        let page = |total| SettlementReturnPage { items: vec![summary()], total };
        let mapped = map_page(page(41), 3, 20, 40).expect("合法页");
        assert_eq!((mapped.pagination.page, mapped.pagination.page_size), (3, 20));
        assert!(map_page(page(40), 3, 20, 40).is_none(), "末行越过总数");
        assert!(map_page(page(1), 1, 0, 0).is_none(), "行数超过页长");
        assert!(
            map_page(SettlementReturnPage { items: Vec::new(), total: -1 }, 1, 20, 0).is_none()
        );
        assert!(
            map_page(SettlementReturnPage { items: Vec::new(), total: 5 }, 9, 20, 160).is_some()
        );
    }

    /// 事件 kind 与逐 kind 细节都在序列化前闭集校验；历史 held 只接受空对象。
    #[test]
    fn event_mapping_rejects_unknown_kinds_and_detail_shapes() {
        let at = Timestamp::from_ymd_hms(2026, 11, 16, 1, 2, 3).expect("时间");
        let make = |event_kind: &str, details| SettlementReturnEvent {
            case_version: 1,
            event_kind: event_kind.to_owned(),
            occurred_at: at,
            actor_id: None,
            command_id: None,
            details,
        };

        let held = map_event(make("held", json!({}))).expect("legacy empty held event");
        assert_eq!(held.event_kind, auction_contracts::AdminSettlementReturnEventKind::Held);
        assert!(map_event(make("held", json!({"next": "hold"}))).is_none());
        assert!(map_event(make("future_event", json!({}))).is_none());
        assert!(
            map_event(make(
                "opened",
                json!({
                    "match_state": "manual_reconcile",
                    "reason_code": "returned_assets_match",
                    "dispatch_no": null,
                    "unexpected": true,
                }),
            ))
            .is_none()
        );
        assert!(
            map_event(make(
                "opened",
                json!({
                    "match_state": "manual_reconcile",
                    "reason_code": "returned_assets_match",
                    "dispatch_no": null,
                }),
            ))
            .is_some()
        );
    }

    /// 详情：动作闭集被允许动作与阻断原因恰好覆盖一次；身份不符、事件乱序或细节不是对象时
    /// 失败关闭。
    #[test]
    fn detail_requires_complete_action_partition_and_ordered_events() {
        let mapped = map_detail(detail(), CASE).expect("合法详情");
        assert_eq!(
            mapped.allowed_actions,
            [
                auction_contracts::AdminSettlementReturnAction::ReclaimAndResend,
                auction_contracts::AdminSettlementReturnAction::ReclaimAndHold,
                auction_contracts::AdminSettlementReturnAction::CloseInbox,
            ]
        );
        assert_eq!(mapped.blockers.len(), 4);
        assert_eq!(
            mapped.blockers[0].action,
            auction_contracts::AdminSettlementReturnAction::ResendFromCustody
        );
        assert_eq!(
            mapped.blockers[0].blocker,
            auction_contracts::AdminSettlementReturnBlocker::StateConflict
        );
        assert_eq!(mapped.events.len(), 2);
        assert_eq!(
            mapped.events[0].event_kind,
            auction_contracts::AdminSettlementReturnEventKind::Opened
        );
        assert_eq!(
            mapped.events[1].event_kind,
            auction_contracts::AdminSettlementReturnEventKind::RecheckRebound
        );

        assert!(map_detail(detail(), Uuid::from_u128(9)).is_none(), "不是请求的案件");
        let mut missing = detail();
        missing.blockers.pop();
        assert!(map_detail(missing, CASE).is_none(), "动作闭集不完整");
        let mut duplicated = detail();
        duplicated.allowed_actions.push(SettlementReturnAction::Recheck);
        assert!(map_detail(duplicated, CASE).is_none(), "同一动作既允许又阻断");
        let mut unordered = detail();
        unordered.events.reverse();
        assert!(map_detail(unordered, CASE).is_none(), "事件版本必须递增");
        let mut details = detail();
        details.events[0].details = json!("text");
        assert!(map_detail(details, CASE).is_none(), "事件细节必须是对象");
    }

    #[test]
    fn migrated_return_detail_preserves_event_provenance() {
        let opened = json!({"backfilled": true});
        let rebound = json!({"migration": 123, "backfill_orphan_ordering_repaired": true});
        let mut migrated = detail();
        migrated.events[0].details = opened.clone();
        migrated.events[1].details = rebound.clone();

        let mapped = map_detail(migrated.clone(), CASE).expect("迁移案件详情");
        assert_eq!(mapped.events[0].details, opened);
        assert_eq!(mapped.events[1].details, rebound);

        let mut mixed = rebound;
        mixed["dispatch_no"] = json!(1);
        for (index, details) in [
            (0, json!({"backfilled": false})),
            (0, json!({"backfilled": true, "dispatch_no": 1})),
            (1, json!({"migration": 124, "backfill_orphan_ordering_repaired": true})),
            (1, json!({"migration": "123", "backfill_orphan_ordering_repaired": true})),
            (1, json!({"migration": 123, "backfill_orphan_ordering_repaired": false})),
            (1, mixed),
        ] {
            let mut malformed = migrated.clone();
            malformed.events[index].details = details;
            assert!(map_detail(malformed, CASE).is_none());
        }
        migrated.events[1].event_kind = "recheck_unchanged".to_owned();
        assert!(map_detail(migrated, CASE).is_none());
    }

    #[test]
    fn take_unknown_history_accepts_persisted_failures_and_observations() {
        let mut unknown = detail();
        let delivery_id = Uuid::from_u128(6);
        let at = unknown.summary.updated_at;
        unknown.summary.status = SettlementReturnCaseStatus::TakeUnknown;
        unknown.summary.asset_location = SettlementReturnAssetLocation::LocationUnknown;
        unknown.summary.case_version = 4;
        unknown.take_unknown_reason = Some(SettlementReturnTakeUnknownReason::MailMissing);
        let failure = json!({
            "failure_code": "settlement_return_take_unknown",
            "delivery_id": delivery_id,
        });
        let observation = json!({
            "observation": "mail_missing",
            "delivery_id": delivery_id,
            "fencing_version": 2,
            "sweep_retries": 0,
        });
        for (case_version, event_kind, details) in
            [(3, "take_unknown", failure.clone()), (4, "take_observed", observation.clone())]
        {
            unknown.events.push(SettlementReturnEvent {
                case_version,
                event_kind: event_kind.to_owned(),
                occurred_at: at,
                actor_id: None,
                command_id: None,
                details,
            });
        }
        let mapped = map_detail(unknown.clone(), CASE).expect("持久化 Take 结果未知与首次观察");
        assert_eq!(mapped.events[2].details, failure);
        assert_eq!(mapped.events[3].details, observation);

        for retries in [json!(-1), json!(9), json!(0.0), json!(1.5), json!("0")] {
            let mut malformed = unknown.clone();
            malformed.events[3].details["sweep_retries"] = retries;
            assert!(map_detail(malformed, CASE).is_none());
        }
        let mut malformed = unknown.clone();
        malformed.events[3].details["observation"] = json!("awaiting_observation");
        assert!(map_detail(malformed, CASE).is_none());

        unknown.events[3].details["sweep_retries"] = json!(8);
        assert!(map_detail(unknown.clone(), CASE).is_some());

        unknown.events.pop();
        unknown.summary.case_version = 3;
        unknown.take_unknown_reason = Some(SettlementReturnTakeUnknownReason::DispositionConflict);
        let conflict = json!({"failure_code": "mail_disposition_conflict", "delivery_id": null});
        unknown.events[2].details = conflict.clone();
        let mapped = map_detail(unknown.clone(), CASE).expect("激活前处置冲突");
        assert_eq!(mapped.events[2].details, conflict);

        unknown.events[2].details["failure_code"] = json!("take_receipt_unproven");
        assert!(map_detail(unknown, CASE).is_none());
    }

    /// 核验能力不可用只随待重新核验的案件输出；其他状态带这个标记是投影不一致，失败关闭。
    #[test]
    fn recheck_unavailable_only_maps_for_recheck_pending() {
        assert!(!map_detail(detail(), CASE).expect("合法详情").recheck_unavailable);
        let mut waiting = detail();
        waiting.summary.status = SettlementReturnCaseStatus::RecheckPending;
        waiting.summary.asset_location = SettlementReturnAssetLocation::Unverified;
        waiting.recheck_unavailable = true;
        assert!(map_detail(waiting, CASE).expect("待核验详情").recheck_unavailable);
        let mut inconsistent = detail();
        inconsistent.recheck_unavailable = true;
        assert!(map_detail(inconsistent, CASE).is_none(), "待决定案件不能带核验能力不可用");
    }

    /// 领回能力不可用只随待领回的案件输出；其他状态带这个标记是投影不一致，失败关闭。
    #[test]
    fn take_unavailable_only_maps_for_take_pending() {
        assert!(!map_detail(detail(), CASE).expect("合法详情").take_unavailable);
        let mut waiting = detail();
        waiting.summary.status = SettlementReturnCaseStatus::TakePending;
        waiting.summary.asset_location = SettlementReturnAssetLocation::TakeInProgress;
        waiting.take_unavailable = true;
        assert!(map_detail(waiting, CASE).expect("待领回详情").take_unavailable);
        let mut inconsistent = detail();
        inconsistent.take_unavailable = true;
        assert!(map_detail(inconsistent, CASE).is_none(), "待决定案件不能带领回能力不可用");
    }

    #[test]
    fn enqueued_take_detail_keeps_unconsumed_terminal_delivery_status() {
        let mut waiting = detail();
        waiting.summary.status = SettlementReturnCaseStatus::TakePending;
        waiting.summary.asset_location = SettlementReturnAssetLocation::TakeInProgress;
        let take_delivery = Uuid::from_u128(6);
        for status in ["succeeded", "failed", "manual_reconcile"] {
            waiting.take = Some(SettlementReturnTake {
                execution_state: "enqueued".to_owned(),
                delivery_id: Some(take_delivery),
                delivery_status: Some(status.to_owned()),
                failure_code: None,
                proven_at: None,
            });
            let mapped = map_detail(waiting.clone(), CASE).expect("待收口的领回详情");
            let take = mapped.take.expect("领回动作");
            assert_eq!(take.execution_state, "enqueued");
            assert_eq!(take.delivery_status.as_deref(), Some(status));
            assert_eq!(take.proven_at, None);
        }
        waiting.take.as_mut().expect("领回动作").delivery_status = None;
        assert!(map_detail(waiting, CASE).is_none(), "已入队动作必须有有效投递状态");
    }

    /// 结果未知的原因只随 `take_unknown` 输出且必须存在；其他状态带原因或结果未知缺原因都是
    /// 投影不一致，失败关闭。
    #[test]
    fn take_unknown_reason_maps_only_for_take_unknown() {
        assert_eq!(map_detail(detail(), CASE).expect("合法详情").take_unknown_reason, None);
        let mut unknown = detail();
        unknown.summary.status = SettlementReturnCaseStatus::TakeUnknown;
        unknown.summary.asset_location = SettlementReturnAssetLocation::LocationUnknown;
        unknown.take_unknown_reason = Some(SettlementReturnTakeUnknownReason::MailMissing);
        assert_eq!(
            map_detail(unknown.clone(), CASE).expect("结果未知详情").take_unknown_reason.as_deref(),
            Some("mail_missing")
        );
        unknown.take_unknown_reason = None;
        assert!(map_detail(unknown, CASE).is_none(), "结果未知必须说明原因");
        let mut inconsistent = detail();
        inconsistent.take_unknown_reason = Some(SettlementReturnTakeUnknownReason::AssetsClaimable);
        assert!(map_detail(inconsistent, CASE).is_none(), "待决定案件不能带结果未知原因");
    }
}
