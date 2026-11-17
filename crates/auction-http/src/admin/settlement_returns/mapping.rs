//! 结算退件应用读模型到 Admin DTO 的映射与安全投影校验。
//!
//! 只输出闭集名称、标识、金额与时间；读模型自相矛盾（分页越界、事件版本乱序、动作集合不完整
//! 等）时返回 `None`，由 handler 按内部错误失败关闭，不把损坏数据交给浏览器。

use std::collections::HashSet;

use auction_application::settlement::{
    SettlementReturnCaseSummary, SettlementReturnCommandResult, SettlementReturnCustody,
    SettlementReturnDetail, SettlementReturnDispatch, SettlementReturnEvent, SettlementReturnPage,
    SettlementReturnTake,
};
use auction_contracts::{
    AdminSettlementReturnBlockerResponse, AdminSettlementReturnCommandResponse,
    AdminSettlementReturnCustodyResponse, AdminSettlementReturnDetailResponse,
    AdminSettlementReturnDispatchResponse, AdminSettlementReturnEventResponse,
    AdminSettlementReturnListResponse, AdminSettlementReturnSummaryResponse,
    AdminSettlementReturnTakeResponse, AdminStrictPaginationResponse,
};
use auction_domain::{SettlementReturnAction, SettlementReturnCaseStatus};
use uuid::Uuid;

use crate::mapping::timestamp;

/// 映射一页列表；`page`、`limit` 与 `offset` 是本次请求的回显与校验依据。
pub(super) fn map_page(
    result: SettlementReturnPage,
    page: i64,
    limit: i64,
    offset: i64,
) -> Option<AdminSettlementReturnListResponse> {
    let count = i64::try_from(result.items.len()).ok()?;
    // 总数与当前页读自同一快照：非空页的末行不可能越过总数。
    if count > limit
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
        status: summary.status.as_str().to_owned(),
        match_state: summary.match_state.as_str().to_owned(),
        reason_code: summary.reason.as_str().to_owned(),
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
        plan: summary.plan.map(|plan| plan.as_str().to_owned()),
        resend_limit: summary.resend_limit,
        resends_used: summary.resends_used,
        asset_location: summary.asset_location.as_str().to_owned(),
        created_at: timestamp(summary.created_at),
        updated_at: timestamp(summary.updated_at),
        closed_at: summary.closed_at.map(timestamp),
    })
}

/// 映射详情；摘要必须正是请求的案件，事件版本严格递增，派发编号严格递增，允许动作与阻断
/// 原因合起来恰好覆盖一次动作闭集。
pub(super) fn map_detail(
    detail: SettlementReturnDetail,
    requested: Uuid,
) -> Option<AdminSettlementReturnDetailResponse> {
    if detail.summary.return_mail_id != requested
        || !strictly_increasing(detail.events.iter().map(|event| event.case_version))
        || !strictly_increasing(
            detail.dispatches.iter().map(|dispatch| i64::from(dispatch.dispatch_no)),
        )
        || detail.events.iter().any(|event| !event.details.is_object())
        // 核验能力不可用只说明待重新核验的案件为何不前进，其他状态出现即投影不一致。
        || (detail.recheck_unavailable
            && detail.summary.status != SettlementReturnCaseStatus::RecheckPending)
        // 领回能力不可用同理只说明待领回的案件为何不前进。
        || (detail.take_unavailable
            && detail.summary.status != SettlementReturnCaseStatus::TakePending)
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
    Some(AdminSettlementReturnDetailResponse {
        summary: map_summary(detail.summary)?,
        events: detail.events.into_iter().map(map_event).collect(),
        dispatches: detail.dispatches.into_iter().map(map_dispatch).collect(),
        take: detail.take.map(map_take),
        resend_dispatch_no: detail.resend_dispatch_no,
        custody: detail.custody.map(map_custody),
        allowed_actions: detail
            .allowed_actions
            .into_iter()
            .map(|action| action.as_str().to_owned())
            .collect(),
        blockers: detail
            .blockers
            .into_iter()
            .map(|(action, blocker)| AdminSettlementReturnBlockerResponse {
                action: action.as_str().to_owned(),
                blocker: blocker.as_str().to_owned(),
            })
            .collect(),
        recheck_unavailable: detail.recheck_unavailable,
        take_unavailable: detail.take_unavailable,
    })
}

/// 映射命令结果。
pub(super) fn map_command_result(
    result: SettlementReturnCommandResult,
) -> AdminSettlementReturnCommandResponse {
    AdminSettlementReturnCommandResponse {
        command_id: result.command_id.as_uuid().to_string(),
        return_id: result.return_mail_id.to_string(),
        action: result.action.as_str().to_owned(),
        case_version: result.case_version,
        status: result.status.as_str().to_owned(),
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

fn map_event(event: SettlementReturnEvent) -> AdminSettlementReturnEventResponse {
    AdminSettlementReturnEventResponse {
        case_version: event.case_version,
        event_kind: event.event_kind,
        occurred_at: timestamp(event.occurred_at),
        actor_id: event.actor_id,
        command_id: event.command_id.map(|command_id| command_id.as_uuid().to_string()),
        details: event.details,
    }
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

#[cfg(test)]
mod tests {
    use auction_application::settlement::{
        SettlementReturnAssetLocation, SettlementReturnCaseSummary, SettlementReturnDetail,
        SettlementReturnEvent, SettlementReturnMatch, SettlementReturnPage, SettlementReturnReason,
    };
    use auction_contracts::{
        ADMIN_SETTLEMENT_RETURN_ACTIONS, ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS,
        ADMIN_SETTLEMENT_RETURN_BLOCKERS, ADMIN_SETTLEMENT_RETURN_MATCH_STATES,
        ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS, ADMIN_SETTLEMENT_RETURN_PLANS,
        ADMIN_SETTLEMENT_RETURN_REASONS, ADMIN_SETTLEMENT_RETURN_STATUSES,
    };
    use auction_domain::{
        SettlementObligationKind, SettlementReturnAction, SettlementReturnBlocker,
        SettlementReturnCaseStatus, SettlementReturnPlan,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use uuid::Uuid;

    use super::{map_detail, map_page, map_summary};

    const CASE: Uuid = Uuid::from_u128(0x0f8f_ad5b_d9cb_469f_a165_7086_7728_950e);

    fn summary() -> SettlementReturnCaseSummary {
        let at = Utc.with_ymd_and_hms(2026, 11, 16, 1, 2, 3).single().expect("时间");
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
        let at = Utc.with_ymd_and_hms(2026, 11, 16, 1, 2, 3).single().expect("时间");
        SettlementReturnDetail {
            summary: summary(),
            events: vec![
                SettlementReturnEvent {
                    case_version: 1,
                    event_kind: "opened".to_owned(),
                    occurred_at: at,
                    actor_id: None,
                    command_id: None,
                    details: json!({"match_state": "manual_reconcile"}),
                },
                SettlementReturnEvent {
                    case_version: 2,
                    event_kind: "recheck_rebound".to_owned(),
                    occurred_at: at,
                    actor_id: None,
                    command_id: None,
                    details: json!({"dispatch_no": 1}),
                },
            ],
            dispatches: Vec::new(),
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
            ],
            recheck_unavailable: false,
            take_unavailable: false,
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
        assert_eq!(mapped.status, "awaiting_decision");
        assert_eq!(mapped.match_state, "manual_reconcile");
        assert_eq!(mapped.reason_code, "returned_assets_match");
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

    /// 详情：动作闭集被允许动作与阻断原因恰好覆盖一次；身份不符、事件乱序或细节不是对象时
    /// 失败关闭。
    #[test]
    fn detail_requires_complete_action_partition_and_ordered_events() {
        let mapped = map_detail(detail(), CASE).expect("合法详情");
        assert_eq!(
            mapped.allowed_actions,
            ["reclaim_and_resend", "reclaim_and_hold", "close_inbox"]
        );
        assert_eq!(mapped.blockers.len(), 3);
        assert_eq!(mapped.blockers[0].action, "resend_from_custody");
        assert_eq!(mapped.blockers[0].blocker, "state_conflict");
        assert_eq!(mapped.events.len(), 2);

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

    /// 核验能力不可用只随待重新核验的案件输出；其他状态带这个标记是投影不一致，失败关闭。
    #[test]
    fn recheck_unavailable_only_maps_for_recheck_pending() {
        assert!(!map_detail(detail(), CASE).expect("合法详情").recheck_unavailable);
        let mut waiting = detail();
        waiting.summary.status = SettlementReturnCaseStatus::RecheckPending;
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
        waiting.take_unavailable = true;
        assert!(map_detail(waiting, CASE).expect("待领回详情").take_unavailable);
        let mut inconsistent = detail();
        inconsistent.take_unavailable = true;
        assert!(map_detail(inconsistent, CASE).is_none(), "待决定案件不能带领回能力不可用");
    }
}
