//! Admin 结算退件案件的同源 HTTP 客户端。
//!
//! 列表与详情只接收服务端安全投影，按契约闭集失败关闭解码：未知状态、动作、原因或资产位置会
//! 让整页读取失败，而不是被浏览器猜测成相近含义。处理命令携带页面命令宿主生成、在重试间保持
//! 不变的 `command_id` 与读取时的案件版本；资格、锁与幂等都由服务端判定。

use std::collections::HashSet;

use auction_contracts::{
    ADMIN_SETTLEMENT_RETURN_ACTIONS, ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS,
    ADMIN_SETTLEMENT_RETURN_BLOCKERS, ADMIN_SETTLEMENT_RETURN_CLOSED_STATUSES,
    ADMIN_SETTLEMENT_RETURN_EVENT_KINDS, ADMIN_SETTLEMENT_RETURN_MATCH_STATES,
    ADMIN_SETTLEMENT_RETURN_MAX_OFFSET, ADMIN_SETTLEMENT_RETURN_MAX_PAGE_SIZE,
    ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS, ADMIN_SETTLEMENT_RETURN_PLANS,
    ADMIN_SETTLEMENT_RETURN_REASONS, ADMIN_SETTLEMENT_RETURN_STATUSES,
    ADMIN_SETTLEMENT_RETURN_TAKE_STATES, AdminSettlementReturnHandleRequest,
    AdminSettlementReturnRecheckRequest, ApiResponse,
};
pub(crate) use auction_contracts::{
    AdminSettlementReturnCommandResponse as SettlementReturnCommandResult,
    AdminSettlementReturnCustodyResponse as SettlementReturnCustody,
    AdminSettlementReturnDetailResponse as SettlementReturnDetail,
    AdminSettlementReturnDispatchResponse as SettlementReturnDispatch,
    AdminSettlementReturnEventResponse as SettlementReturnEvent,
    AdminSettlementReturnListResponse as SettlementReturnPage,
    AdminSettlementReturnSummaryResponse as SettlementReturnSummary,
    AdminSettlementReturnTakeResponse as SettlementReturnTake,
};
use gloo_net::http::Request;
use serde::Serialize;
use uuid::Uuid;

use super::{
    AdminApiFailure, MAX_JSON_SAFE_INTEGER, fetch_csrf_token, invalid_request_error,
    invalid_response_error, response_error, response_failure, send_get, send_request,
    valid_admin_time, valid_count, valid_optional_time, valid_safe_text, valid_status,
    valid_success_code, valid_uuid,
};

/// 列表筛选；状态、分类、种类与世界都取契约闭集中的值，空串表示不限。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SettlementReturnFilters {
    /// 只列未结案案件。
    pub(crate) open_only: bool,
    pub(crate) status: String,
    pub(crate) match_state: String,
    pub(crate) obligation_kind: String,
    pub(crate) world: String,
}

/// 读取一页结算退件案件。
pub(crate) async fn fetch_settlement_returns(
    filters: SettlementReturnFilters,
    page: i64,
    page_size: i64,
) -> Result<SettlementReturnPage, String> {
    super::pagination::validate(
        page,
        page_size,
        ADMIN_SETTLEMENT_RETURN_MAX_PAGE_SIZE,
        ADMIN_SETTLEMENT_RETURN_MAX_OFFSET,
    )?;
    let url = list_url(&filters, page, page_size).ok_or_else(|| "invalid_request".to_owned())?;
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<SettlementReturnPage> =
        response.json().await.map_err(invalid_response_error)?;
    if !valid_success_code(&payload.code) || !valid_page(&payload.data, &filters, page, page_size) {
        return Err(invalid_response_error("settlement return page"));
    }
    Ok(payload.data)
}

/// 读取单个案件详情；响应必须正是请求的案件。
pub(crate) async fn fetch_settlement_return(
    return_id: String,
) -> Result<SettlementReturnDetail, String> {
    if !valid_uuid(&return_id) {
        return Err("invalid_settlement_return_id".to_owned());
    }
    let response = send_get(&format!("/api/v1/admin/settlement-returns/{return_id}")).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<SettlementReturnDetail> =
        response.json().await.map_err(invalid_response_error)?;
    if !valid_success_code(&payload.code) || !valid_detail(&payload.data, &return_id) {
        return Err(invalid_response_error("settlement return detail"));
    }
    Ok(payload.data)
}

/// 提交处理动作；`recheck` 走只读重新核验路由，其余动作走 `handle`。同一 `command_id` 原样重试
/// 由服务端重放首次结果。
pub(crate) async fn submit_settlement_return_action(
    command_id: Uuid,
    return_id: String,
    expected_version: i64,
    action: String,
) -> Result<SettlementReturnCommandResult, AdminApiFailure> {
    if command_id.is_nil()
        || !valid_uuid(&return_id)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&expected_version)
        || !ADMIN_SETTLEMENT_RETURN_ACTIONS.contains(&action.as_str())
    {
        return Err(AdminApiFailure::client("invalid_settlement_return_command"));
    }
    let result = if action == "recheck" {
        post_command(
            &format!("/api/v1/admin/settlement-returns/{return_id}/recheck"),
            &AdminSettlementReturnRecheckRequest { command_id, expected_version },
        )
        .await?
    } else {
        post_command(
            &format!("/api/v1/admin/settlement-returns/{return_id}/handle"),
            &AdminSettlementReturnHandleRequest {
                command_id,
                expected_version,
                action: action.clone(),
            },
        )
        .await?
    };
    if !valid_command_result(&result, command_id, &return_id, &action, expected_version) {
        return Err(AdminApiFailure::client(invalid_response_error("settlement return command")));
    }
    Ok(result)
}

async fn post_command<T: Serialize>(
    url: &str,
    request: &T,
) -> Result<SettlementReturnCommandResult, AdminApiFailure> {
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let response = send_request(|abort_signal| {
        Request::post(url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(request)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<SettlementReturnCommandResult> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    if !valid_success_code(&payload.code) {
        return Err(AdminApiFailure::client(invalid_response_error("settlement return command")));
    }
    Ok(payload.data)
}

/// 列表请求地址；筛选只接受契约闭集取值，闭集外的值返回 `None` 而不是被拼进查询串。
pub(crate) fn list_url(
    filters: &SettlementReturnFilters,
    page: i64,
    page_size: i64,
) -> Option<String> {
    let mut url = format!("/api/v1/admin/settlement-returns?page={page}&page_size={page_size}");
    if filters.open_only {
        url.push_str("&open_only=true");
    }
    for (key, value, allowed) in [
        ("status", &filters.status, ADMIN_SETTLEMENT_RETURN_STATUSES.as_slice()),
        ("match_state", &filters.match_state, ADMIN_SETTLEMENT_RETURN_MATCH_STATES.as_slice()),
        (
            "obligation_kind",
            &filters.obligation_kind,
            ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS.as_slice(),
        ),
        ("world", &filters.world, ["main", "isekai"].as_slice()),
    ] {
        if value.is_empty() {
            continue;
        }
        if !allowed.contains(&value.as_str()) {
            return None;
        }
        url.push_str(&format!("&{key}={value}"));
    }
    Some(url)
}

/// 分页回显、每行摘要与筛选回显都必须成立：服务端不能返回不在筛选内的案件。
pub(crate) fn valid_page(
    data: &SettlementReturnPage,
    filters: &SettlementReturnFilters,
    page: i64,
    page_size: i64,
) -> bool {
    let mut ids = HashSet::with_capacity(data.returns.len());
    super::pagination::valid_page_echo(
        page,
        page_size,
        data.pagination.page,
        data.pagination.page_size,
        data.pagination.total,
        data.returns.len(),
    ) && data.returns.iter().all(|item| {
        ids.insert(item.return_id.as_str())
            && valid_summary(item)
            && (!filters.open_only || item.closed_at.is_none())
            && (filters.status.is_empty() || item.status == filters.status)
            && (filters.match_state.is_empty() || item.match_state == filters.match_state)
            && (filters.obligation_kind.is_empty()
                || item.obligation_kind.as_deref() == Some(filters.obligation_kind.as_str()))
            && (filters.world.is_empty() || item.world_id == filters.world)
    })
}

/// 案件摘要的闭集与形状校验；结案时刻与已结案状态必须一致。
pub(crate) fn valid_summary(item: &SettlementReturnSummary) -> bool {
    let closed = ADMIN_SETTLEMENT_RETURN_CLOSED_STATUSES.contains(&item.status.as_str());
    valid_uuid(&item.return_id)
        && (1..=MAX_JSON_SAFE_INTEGER).contains(&item.case_version)
        && ADMIN_SETTLEMENT_RETURN_STATUSES.contains(&item.status.as_str())
        && ADMIN_SETTLEMENT_RETURN_MATCH_STATES.contains(&item.match_state.as_str())
        && ADMIN_SETTLEMENT_RETURN_REASONS.contains(&item.reason_code.as_str())
        && matches!(item.world_id.as_str(), "main" | "isekai")
        && valid_safe_text(&item.external_mail_id, 256)
        && valid_admin_time(&item.received_at)
        && item.obligation_id.as_deref().is_none_or(valid_uuid)
        && item
            .obligation_kind
            .as_deref()
            .is_none_or(|kind| ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS.contains(&kind))
        && item.auction_id.as_deref().is_none_or(valid_uuid)
        && item.lot_id.as_deref().is_none_or(valid_uuid)
        && item.delivery_id.as_deref().is_none_or(valid_uuid)
        && item.dispatch_no.is_none_or(|value| value >= 1)
        && item.recipient.as_deref().is_none_or(|value| valid_safe_text(value, 256))
        && valid_count(item.credits)
        && valid_count(item.cod)
        && item.plan.as_deref().is_none_or(|plan| ADMIN_SETTLEMENT_RETURN_PLANS.contains(&plan))
        && ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS.contains(&item.asset_location.as_str())
        && valid_admin_time(&item.created_at)
        && valid_admin_time(&item.updated_at)
        && valid_optional_time(item.closed_at.as_deref())
        && item.closed_at.is_some() == closed
}

/// 详情：摘要正是请求的案件；事件与派发按版本/编号严格递增；动作闭集被允许动作与阻断原因恰好
/// 覆盖一次。
pub(crate) fn valid_detail(detail: &SettlementReturnDetail, requested: &str) -> bool {
    let mut actions = HashSet::with_capacity(ADMIN_SETTLEMENT_RETURN_ACTIONS.len());
    let partition = detail
        .allowed_actions
        .iter()
        .map(String::as_str)
        .chain(detail.blockers.iter().map(|blocker| blocker.action.as_str()))
        .all(|action| ADMIN_SETTLEMENT_RETURN_ACTIONS.contains(&action) && actions.insert(action))
        && actions.len() == ADMIN_SETTLEMENT_RETURN_ACTIONS.len()
        && detail
            .blockers
            .iter()
            .all(|blocker| ADMIN_SETTLEMENT_RETURN_BLOCKERS.contains(&blocker.blocker.as_str()));
    valid_summary(&detail.summary)
        && detail.summary.return_id == requested
        && partition
        && increasing(detail.events.iter().map(|event| event.case_version))
        && detail.events.iter().all(valid_event)
        && increasing(detail.dispatches.iter().map(|dispatch| i64::from(dispatch.dispatch_no)))
        && detail.dispatches.iter().all(valid_dispatch)
        && detail.take.as_ref().is_none_or(valid_take)
        && detail.resend_dispatch_no.is_none_or(|value| value >= 2)
        && detail.custody.as_ref().is_none_or(valid_custody)
        && (!detail.recheck_unavailable || detail.summary.status == "recheck_pending")
        && (!detail.take_unavailable || detail.summary.status == "take_pending")
}

/// 命令结果必须对应本次命令身份、案件与动作，版本只会前进。
pub(crate) fn valid_command_result(
    result: &SettlementReturnCommandResult,
    command_id: Uuid,
    return_id: &str,
    action: &str,
    expected_version: i64,
) -> bool {
    result.command_id == command_id.to_string()
        && result.return_id == return_id
        && result.action == action
        && result.case_version > expected_version
        && result.case_version <= MAX_JSON_SAFE_INTEGER
        && ADMIN_SETTLEMENT_RETURN_STATUSES.contains(&result.status.as_str())
}

fn increasing(mut values: impl Iterator<Item = i64>) -> bool {
    let mut previous = 0;
    values.all(|value| {
        let ordered = value > previous;
        previous = value;
        ordered
    })
}

fn valid_event(event: &SettlementReturnEvent) -> bool {
    ADMIN_SETTLEMENT_RETURN_EVENT_KINDS.contains(&event.event_kind.as_str())
        && valid_admin_time(&event.occurred_at)
        && event.actor_id.as_deref().is_none_or(|actor| valid_safe_text(actor, 128))
        && event.command_id.as_deref().is_none_or(valid_uuid)
        && event.details.is_object()
}

fn valid_dispatch(dispatch: &SettlementReturnDispatch) -> bool {
    valid_uuid(&dispatch.delivery_id)
        && dispatch.delivery_status.as_deref().is_none_or(valid_status)
        && valid_optional_time(dispatch.sending_at.as_deref())
        && valid_optional_time(dispatch.completed_at.as_deref())
        && dispatch.authorized_by.as_deref().is_none_or(valid_uuid)
        && (dispatch.dispatch_no == 1) == dispatch.authorized_by.is_none()
}

fn valid_take(take: &SettlementReturnTake) -> bool {
    ADMIN_SETTLEMENT_RETURN_TAKE_STATES.contains(&take.execution_state.as_str())
        && take.delivery_id.as_deref().is_none_or(valid_uuid)
        && take.delivery_status.as_deref().is_none_or(valid_status)
        && take.failure_code.as_deref().is_none_or(|code| valid_safe_text(code, 96))
        && valid_optional_time(take.proven_at.as_deref())
}

fn valid_custody(custody: &SettlementReturnCustody) -> bool {
    valid_safe_text(&custody.beneficiary, 256)
        && valid_count(custody.credits)
        && custody
            .equipment_ids
            .iter()
            .all(|id| (1..=32).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::{
        SettlementReturnCommandResult, SettlementReturnDetail, SettlementReturnFilters,
        SettlementReturnPage, SettlementReturnSummary, list_url, valid_command_result,
        valid_detail, valid_page, valid_summary,
    };

    const CASE: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const DELIVERY: &str = "1f8fad5b-d9cb-469f-a165-70867728950e";

    fn summary_json() -> Value {
        json!({
            "return_id": CASE,
            "case_version": 2,
            "status": "awaiting_decision",
            "match_state": "manual_reconcile",
            "reason_code": "returned_assets_match",
            "world_id": "main",
            "external_mail_id": "993021",
            "received_at": "2026-11-16T01:02:03.000000Z",
            "obligation_id": "2f8fad5b-d9cb-469f-a165-70867728950e",
            "obligation_kind": "seller_payout",
            "auction_id": null,
            "lot_id": null,
            "delivery_id": DELIVERY,
            "dispatch_no": 1,
            "recipient": "Seller",
            "credits": 12000,
            "cod": 0,
            "attachment_count": 0,
            "plan": null,
            "resend_limit": 3,
            "resends_used": 0,
            "asset_location": "returned_in_inbox",
            "created_at": "2026-11-16T01:02:03.000000Z",
            "updated_at": "2026-11-16T01:02:03.000000Z",
            "closed_at": null
        })
    }

    fn summary() -> SettlementReturnSummary {
        serde_json::from_value(summary_json()).expect("摘要夹具")
    }

    fn detail_json() -> Value {
        json!({
            "summary": summary_json(),
            "events": [
                {"case_version": 1, "event_kind": "opened", "occurred_at": "2026-11-16T01:02:03Z",
                 "actor_id": null, "command_id": null, "details": {"dispatch_no": 1}},
                {"case_version": 2, "event_kind": "recheck_rebound",
                 "occurred_at": "2026-11-16T01:03:03Z", "actor_id": null, "command_id": null,
                 "details": {}}
            ],
            "dispatches": [
                {"dispatch_no": 1, "delivery_id": DELIVERY, "delivery_status": "succeeded",
                 "sending_at": "2026-11-16T00:00:00Z", "completed_at": "2026-11-16T00:01:00Z",
                 "authorized_by": null}
            ],
            "take": null,
            "resend_dispatch_no": null,
            "custody": null,
            "allowed_actions": ["reclaim_and_resend", "reclaim_and_hold", "close_inbox"],
            "blockers": [
                {"action": "resend_from_custody", "blocker": "state_conflict"},
                {"action": "acknowledge", "blocker": "state_conflict"},
                {"action": "recheck", "blocker": "state_conflict"}
            ],
            "recheck_unavailable": false,
            "take_unavailable": false
        })
    }

    fn detail_with(change: impl FnOnce(&mut Value)) -> SettlementReturnDetail {
        let mut value = detail_json();
        change(&mut value);
        serde_json::from_value(value).expect("详情夹具")
    }

    fn page(items: Vec<SettlementReturnSummary>, total: i64) -> SettlementReturnPage {
        serde_json::from_value(json!({
            "returns": items,
            "pagination": {"page": 1, "page_size": 20, "total": total}
        }))
        .expect("分页夹具")
    }

    /// 请求地址只拼入闭集取值；闭集外的筛选不发请求。
    #[test]
    fn list_url_accepts_only_contract_filters() {
        let filters = SettlementReturnFilters {
            open_only: true,
            status: "take_unknown".to_owned(),
            match_state: String::new(),
            obligation_kind: "buyer_refund".to_owned(),
            world: "isekai".to_owned(),
        };
        assert_eq!(
            list_url(&filters, 2, 50).as_deref(),
            Some(
                "/api/v1/admin/settlement-returns?page=2&page_size=50&open_only=true\
                 &status=take_unknown&obligation_kind=buyer_refund&world=isekai"
            )
        );
        let default = SettlementReturnFilters::default();
        assert_eq!(
            list_url(&default, 1, 20).as_deref(),
            Some("/api/v1/admin/settlement-returns?page=1&page_size=20")
        );
        for invalid in [
            SettlementReturnFilters { status: "open".to_owned(), ..Default::default() },
            SettlementReturnFilters { world: "other".to_owned(), ..Default::default() },
            SettlementReturnFilters { match_state: "bound".to_owned(), ..Default::default() },
        ] {
            assert_eq!(list_url(&invalid, 1, 20), None, "{invalid:?}");
        }
    }

    /// 摘要闭集失败关闭；结案时刻必须与已结案状态一致。
    #[test]
    fn summary_rejects_unknown_names_and_inconsistent_closure() {
        assert!(valid_summary(&summary()));
        for (key, value) in [
            ("status", json!("open")),
            ("asset_location", json!("in_bot")),
            ("reason_code", json!("other")),
            ("world_id", json!("third")),
            ("credits", json!(-1)),
            ("closed_at", json!("2026-11-16T02:00:00Z")),
        ] {
            let mut item = summary_json();
            item[key] = value;
            let parsed: SettlementReturnSummary = serde_json::from_value(item).expect("可解码");
            assert!(!valid_summary(&parsed), "{key} 应被拒绝");
        }
        let mut held = summary_json();
        held["status"] = json!("held");
        assert!(
            !valid_summary(&serde_json::from_value(held.clone()).expect("可解码")),
            "已结案状态必须带结案时刻"
        );
        held["closed_at"] = json!("2026-11-16T02:00:00Z");
        assert!(valid_summary(&serde_json::from_value(held).expect("可解码")));
    }

    /// 列表行必须落在所选筛选内，且不重复。
    #[test]
    fn page_rows_must_match_filters_and_echo() {
        let open = SettlementReturnFilters { open_only: true, ..Default::default() };
        assert!(valid_page(&page(vec![summary()], 1), &open, 1, 20));
        assert!(!valid_page(&page(vec![summary(), summary()], 2), &open, 1, 20), "重复案件");
        let held = SettlementReturnFilters { status: "held".to_owned(), ..Default::default() };
        assert!(!valid_page(&page(vec![summary()], 1), &held, 1, 20), "状态不在筛选内");
        let isekai = SettlementReturnFilters { world: "isekai".to_owned(), ..Default::default() };
        assert!(!valid_page(&page(vec![summary()], 1), &isekai, 1, 20), "世界不在筛选内");
        assert!(!valid_page(&page(vec![summary()], 1), &open, 2, 20), "页码未回显");
        let mut closed = summary_json();
        closed["status"] = json!("reviewed");
        closed["closed_at"] = json!("2026-11-16T02:00:00Z");
        let closed: SettlementReturnSummary = serde_json::from_value(closed).expect("可解码");
        assert!(!valid_page(&page(vec![closed], 1), &open, 1, 20), "只看未结案时不能有已结案");
    }

    /// 详情：动作闭集恰好覆盖一次；事件版本递增；首寄没有授权案件、补寄必须有。
    #[test]
    fn detail_requires_partition_order_and_dispatch_shape() {
        assert!(valid_detail(&detail_with(|_| {}), CASE));
        assert!(!valid_detail(&detail_with(|_| {}), DELIVERY), "不是请求的案件");
        let missing = detail_with(|value| {
            if let Some(blockers) = value["blockers"].as_array_mut() {
                blockers.pop();
            }
        });
        assert!(!valid_detail(&missing, CASE), "动作闭集不完整");
        let duplicated = detail_with(|value| {
            value["allowed_actions"] =
                json!(["reclaim_and_resend", "reclaim_and_hold", "close_inbox", "recheck"]);
        });
        assert!(!valid_detail(&duplicated, CASE), "同一动作既允许又阻断");
        let unknown = detail_with(|value| {
            value["blockers"][0]["blocker"] = json!("busy");
        });
        assert!(!valid_detail(&unknown, CASE), "未知阻断原因");
        let unordered = detail_with(|value| {
            value["events"][1]["case_version"] = json!(1);
        });
        assert!(!valid_detail(&unordered, CASE), "事件版本必须递增");
        let resend_without_case = detail_with(|value| {
            value["dispatches"][0]["dispatch_no"] = json!(2);
        });
        assert!(!valid_detail(&resend_without_case, CASE), "补寄必须引用授权案件");
        let custody = detail_with(|value| {
            value["custody"] =
                json!({"beneficiary": "Seller", "credits": 12000, "equipment_ids": ["12a"]});
        });
        assert!(!valid_detail(&custody, CASE), "装备 EID 只能是十进制数字");
        let unavailable = detail_with(|value| {
            value["recheck_unavailable"] = json!(true);
        });
        assert!(!valid_detail(&unavailable, CASE), "核验能力不可用只随待重新核验的案件");
        let waiting = detail_with(|value| {
            value["summary"]["status"] = json!("recheck_pending");
            value["recheck_unavailable"] = json!(true);
        });
        assert!(valid_detail(&waiting, CASE), "待重新核验的案件可以说明核验能力不可用");
        let take_unavailable = detail_with(|value| {
            value["take_unavailable"] = json!(true);
        });
        assert!(!valid_detail(&take_unavailable, CASE), "领回能力不可用只随待领回的案件");
        let take_waiting = detail_with(|value| {
            value["summary"]["status"] = json!("take_pending");
            value["take_unavailable"] = json!(true);
        });
        assert!(valid_detail(&take_waiting, CASE), "待领回的案件可以说明领回能力不可用");
    }

    /// 命令结果必须对应本次命令身份、案件与动作，版本只会前进。
    #[test]
    fn command_result_must_echo_the_request() {
        let command_id = Uuid::from_u128(7);
        let result: SettlementReturnCommandResult = serde_json::from_value(json!({
            "command_id": command_id.to_string(),
            "return_id": CASE,
            "action": "close_inbox",
            "case_version": 3,
            "status": "inbox_closed"
        }))
        .expect("结果夹具");
        assert!(valid_command_result(&result, command_id, CASE, "close_inbox", 2));
        assert!(!valid_command_result(&result, Uuid::from_u128(8), CASE, "close_inbox", 2));
        assert!(!valid_command_result(&result, command_id, DELIVERY, "close_inbox", 2));
        assert!(!valid_command_result(&result, command_id, CASE, "reclaim_and_hold", 2));
        assert!(!valid_command_result(&result, command_id, CASE, "close_inbox", 3));
    }
}
