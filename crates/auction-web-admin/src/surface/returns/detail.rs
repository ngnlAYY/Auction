//! 结算退件详情页：摘要与资产位置说明、处理动作与阻断原因、领回（Take）、代管、派发链与事件。
//!
//! 字段全部来自服务端安全投影；按钮只按服务端给出的 `allowed_actions` 展示，命令事务仍会锁内
//! 复核。页面明确区分“已寄出”（邮件发出）、“退回仍在收件箱”、“已领回”与“代管”。

use auction_web_shared::{AsyncState, AsyncStateKind, Time, TruncatedText, t};
use dioxus::prelude::*;

use super::actions::ActionSection;
use super::labels::{
    event_details_text, event_key, kind_key, label, location_key, match_key, plan_key, reason_key,
    status_key, take_state_key, take_unknown_guidance_key, take_unknown_reason_key,
};
use super::list::assets_text;
use crate::api::{
    SettlementReturnCustody, SettlementReturnDispatch, SettlementReturnEvent,
    SettlementReturnSummary, SettlementReturnTake, fetch_settlement_return, valid_uuid,
};
use crate::surface::deliveries::labels::delivery_status_label;
use crate::surface::pending::{PendingCommandPanel, use_pending_commands};
use crate::surface::router::Route;
use crate::surface::{AdminActionFeedback, admin_api_error_message, admin_world_label};

/// 路由复用同一组件实例，按案件身份重挂载资源与一次性状态。
pub(crate) fn detail_page(return_id: String) -> Element {
    rsx! {
        for return_id in [return_id] {
            ReturnDetailPage { key: "{return_id}", return_id }
        }
    }
}

#[component]
fn ReturnDetailPage(return_id: String) -> Element {
    let mut refresh = use_signal(|| 0_u64);
    // 命令成功后宿主递增 refresh 重读详情；结果未知的命令留在未决区按原 command_id 重试。
    let commands = use_pending_commands(refresh);
    let notice = use_signal(|| None::<AdminActionFeedback>);
    let detail = use_resource(use_reactive((&return_id,), move |(return_id,)| {
        let _ = refresh();
        async move {
            let result = fetch_settlement_return(return_id.clone()).await;
            (return_id, result)
        }
    }));
    let loading = detail.state().cloned() != UseResourceState::Ready;
    // 刷新期间不展示上一轮结果，避免按旧版本提交命令。
    let loaded = if loading {
        None
    } else {
        detail
            .read()
            .as_ref()
            .filter(|(loaded_id, _)| loaded_id == &return_id)
            .map(|(_, result)| result.clone())
    };
    rsx! {
        section { class: "admin-settlement-return-detail", aria_labelledby: "settlement-return-detail-title",
            div { class: "admin-settlement-return-detail__head",
                h1 { id: "settlement-return-detail-title", {t!("settlement-return-detail-title")} }
                Link { to: Route::SettlementReturns {}, class: "admin-settlement-return-detail__back",
                    {t!("settlement-return-detail-back")}
                }
            }
            PendingCommandPanel { owner: commands }
            match loaded {
                None if !valid_uuid(&return_id) => rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Error,
                        title: admin_api_error_message("invalid_settlement_return_id"),
                        detail: "",
                    }
                },
                None => rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Loading,
                        title: t!("settlement-return-detail-loading"),
                        detail: "",
                    }
                },
                Some(Err(error)) => rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Error,
                        title: admin_api_error_message(&error),
                        detail: "",
                        retry_label: t!("admin-retry"),
                        on_retry: move |_| refresh.set(refresh().saturating_add(1)),
                    }
                },
                Some(Ok(detail)) => rsx! {
                    // 待重新核验但没有能读取该世界 MoogleMail 的在线 Worker：说明案件为何不前进。
                    if detail.recheck_unavailable {
                        p { class: "action-feedback action-feedback--error", role: "note",
                            {t!("settlement-return-recheck-unavailable")}
                        }
                    }
                    // 待领回但没有 Worker 能以数据库凭据身份执行该世界的 Take：说明案件为何不前进。
                    if detail.take_unavailable {
                        p { class: "action-feedback action-feedback--error", role: "note",
                            {t!("settlement-return-take-unavailable")}
                        }
                    }
                    SummarySection { summary: detail.summary.clone() }
                    ActionSection { detail: detail.clone(), notice, refresh }
                    TakeSection {
                        take: detail.take.clone(),
                        status: detail.summary.status.clone(),
                        unknown_reason: detail.take_unknown_reason.clone(),
                    }
                    if let Some(custody) = detail.custody.clone() {
                        CustodySection { custody }
                    }
                    DispatchSection {
                        dispatches: detail.dispatches.clone(),
                        returned_delivery: detail.summary.delivery_id.clone(),
                        resend_dispatch_no: detail.resend_dispatch_no,
                    }
                    EventSection { events: detail.events }
                },
            }
        }
    }
}

#[component]
fn SummarySection(summary: SettlementReturnSummary) -> Element {
    let empty = || t!("value-empty");
    let kind = summary.obligation_kind.as_deref().map_or_else(empty, |kind| label(kind_key, kind));
    let plan = summary.plan.as_deref().map_or_else(empty, |plan| label(plan_key, plan));
    let resends = summary.resends_used.map_or_else(empty, |used| {
        t!("settlement-return-resends-value", used: used.to_string(), limit: summary.resend_limit.to_string())
    });
    let dispatch = summary.dispatch_no.map_or_else(empty, |value| value.to_string());
    rsx! {
        section { class: "admin-panel", aria_labelledby: "settlement-return-summary-title",
            h2 { id: "settlement-return-summary-title", {t!("settlement-return-summary-title")} }
            p { class: "admin-panel__note", {t!("settlement-return-location-note")} }
            dl { class: "admin-delivery-detail__grid",
                dt { {t!("settlement-return-field-location")} }
                dd { strong { {label(location_key, &summary.asset_location)} } }
                dt { {t!("settlement-return-column-status")} }
                dd { {label(status_key, &summary.status)} }
                dt { {t!("settlement-return-field-id")} }
                dd { TruncatedText { value: summary.return_id.clone(), portal: true } }
                dt { {t!("settlement-return-field-version")} }
                dd { "{summary.case_version}" }
                dt { {t!("settlement-return-column-match")} }
                dd { {label(match_key, &summary.match_state)} }
                dt { {t!("settlement-return-field-reason")} }
                dd { {label(reason_key, &summary.reason_code)} }
                dt { {t!("settlement-return-column-world")} }
                dd { {admin_world_label(&summary.world_id)} }
                dt { {t!("settlement-return-field-mid")} }
                dd { "{summary.external_mail_id}" }
                dt { {t!("settlement-return-column-received")} }
                dd { Time { value: summary.received_at.clone() } }
                dt { {t!("settlement-return-column-kind")} }
                dd { "{kind}" }
                dt { {t!("settlement-return-field-obligation")} }
                dd { TruncatedText { value: summary.obligation_id.clone().unwrap_or_else(empty), portal: true } }
                dt { {t!("settlement-return-field-returned-delivery")} }
                dd {
                    if let Some(delivery_id) = summary.delivery_id.clone() {
                        Link { to: Route::DeliveryDetail { delivery_id: delivery_id.clone() },
                            TruncatedText { value: delivery_id, portal: true }
                        }
                    } else {
                        {empty()}
                    }
                }
                dt { {t!("settlement-return-field-dispatch")} }
                dd { "{dispatch}" }
                dt { {t!("settlement-return-column-recipient")} }
                dd { TruncatedText { value: summary.recipient.clone().unwrap_or_else(empty), portal: true } }
                dt { {t!("settlement-return-column-assets")} }
                dd { {assets_text(&summary)} }
                dt { {t!("settlement-return-field-plan")} }
                dd { "{plan}" }
                dt { {t!("settlement-return-field-resends")} }
                dd { "{resends}" }
                dt { {t!("settlement-return-field-created")} }
                dd { Time { value: summary.created_at.clone() } }
                dt { {t!("settlement-return-field-updated")} }
                dd { Time { value: summary.updated_at.clone() } }
                dt { {t!("settlement-return-field-closed")} }
                dd {
                    if let Some(closed_at) = summary.closed_at {
                        Time { value: closed_at }
                    } else {
                        {empty()}
                    }
                }
            }
        }
    }
}

#[component]
fn TakeSection(
    take: Option<SettlementReturnTake>,
    status: String,
    unknown_reason: Option<String>,
) -> Element {
    let empty = || t!("value-empty");
    rsx! {
        section { class: "admin-panel", aria_labelledby: "settlement-return-take-title",
            h2 { id: "settlement-return-take-title", {t!("settlement-return-take-title")} }
            if status == "take_unknown" {
                p { class: "action-feedback action-feedback--error", role: "note",
                    {t!("settlement-return-take-unknown-warning")}
                }
            }
            // 结果未知的具体原因（最近一次原会话观察或由持久事实推出）与对应处置指引。
            if let Some(reason) = unknown_reason {
                p { class: "admin-panel__note", role: "note",
                    {t!("settlement-return-take-unknown-reason", reason: label(take_unknown_reason_key, &reason))}
                    " "
                    {label(take_unknown_guidance_key, &reason)}
                }
            }
            if let Some(take) = take {
                dl { class: "admin-delivery-detail__grid",
                    dt { {t!("settlement-return-take-state")} }
                    dd { {label(take_state_key, &take.execution_state)} }
                    dt { {t!("settlement-return-take-delivery")} }
                    dd {
                        if let Some(delivery_id) = take.delivery_id.clone() {
                            Link { to: Route::DeliveryDetail { delivery_id: delivery_id.clone() },
                                TruncatedText { value: delivery_id, portal: true }
                            }
                        } else {
                            {empty()}
                        }
                    }
                    dt { {t!("settlement-return-take-delivery-status")} }
                    dd { {take.delivery_status.as_deref().map_or_else(empty, delivery_status_label)} }
                    dt { {t!("settlement-return-take-failure")} }
                    dd { {take.failure_code.clone().unwrap_or_else(empty)} }
                    dt { {t!("settlement-return-take-proven")} }
                    dd {
                        if let Some(proven_at) = take.proven_at {
                            Time { value: proven_at }
                        } else {
                            {empty()}
                        }
                    }
                }
            } else {
                p { class: "admin-panel__note", {t!("settlement-return-take-none")} }
            }
        }
    }
}

#[component]
fn CustodySection(custody: SettlementReturnCustody) -> Element {
    let equipment = if custody.equipment_ids.is_empty() {
        t!("value-empty")
    } else {
        custody.equipment_ids.join(", ")
    };
    rsx! {
        section { class: "admin-panel", aria_labelledby: "settlement-return-custody-title",
            h2 { id: "settlement-return-custody-title", {t!("settlement-return-custody-title")} }
            p { class: "admin-panel__note", {t!("settlement-return-custody-note")} }
            dl { class: "admin-delivery-detail__grid",
                dt { {t!("settlement-return-custody-beneficiary")} }
                dd { TruncatedText { value: custody.beneficiary.clone(), portal: true } }
                dt { {t!("settlement-return-custody-credits")} }
                dd { "{custody.credits}" }
                dt { {t!("settlement-return-custody-equipment")} }
                dd { TruncatedText { value: equipment, size: "wide", portal: true } }
            }
        }
    }
}

#[component]
fn DispatchSection(
    dispatches: Vec<SettlementReturnDispatch>,
    returned_delivery: Option<String>,
    resend_dispatch_no: Option<u32>,
) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "settlement-return-dispatches-title",
            h2 { id: "settlement-return-dispatches-title", {t!("settlement-return-dispatches-title")} }
            p { class: "admin-panel__note", {t!("settlement-return-dispatches-note")} }
            if dispatches.is_empty() {
                p { class: "admin-panel__note", {t!("settlement-return-dispatches-empty")} }
            } else {
                div { class: "admin-shell__table-wrap",
                    table { class: "admin-shell__table",
                        aria_label: t!("settlement-return-dispatches-title"),
                        thead {
                            tr {
                                th { scope: "col", {t!("settlement-return-dispatch-no")} }
                                th { scope: "col", {t!("settlement-return-dispatch-delivery")} }
                                th { scope: "col", {t!("settlement-return-dispatch-status")} }
                                th { scope: "col", {t!("settlement-return-dispatch-sending")} }
                                th { scope: "col", {t!("settlement-return-dispatch-completed")} }
                                th { scope: "col", {t!("settlement-return-dispatch-authorized")} }
                            }
                        }
                        tbody {
                            for dispatch in dispatches {
                                DispatchRow {
                                    key: "{dispatch.dispatch_no}",
                                    returned: returned_delivery.as_deref() == Some(dispatch.delivery_id.as_str()),
                                    resend: resend_dispatch_no == Some(dispatch.dispatch_no),
                                    dispatch,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DispatchRow(dispatch: SettlementReturnDispatch, returned: bool, resend: bool) -> Element {
    let empty = || t!("value-empty");
    let status = dispatch.delivery_status.as_deref().map_or_else(empty, delivery_status_label);
    let number = if dispatch.dispatch_no == 1 {
        t!("settlement-return-dispatch-first", number: dispatch.dispatch_no.to_string())
    } else {
        t!("settlement-return-dispatch-resend", number: dispatch.dispatch_no.to_string())
    };
    rsx! {
        tr {
            td {
                "{number}"
                if returned {
                    " "
                    span { class: "admin-dashboard-badge admin-dashboard-badge--warn",
                        {t!("settlement-return-dispatch-returned")}
                    }
                }
                if resend {
                    " "
                    span { class: "admin-dashboard-badge admin-dashboard-badge--neutral",
                        {t!("settlement-return-dispatch-this-case")}
                    }
                }
            }
            td {
                Link { to: Route::DeliveryDetail { delivery_id: dispatch.delivery_id.clone() },
                    TruncatedText { value: dispatch.delivery_id.clone(), portal: true }
                }
            }
            td { "{status}" }
            td {
                if let Some(sending_at) = dispatch.sending_at.clone() {
                    Time { value: sending_at }
                } else {
                    {empty()}
                }
            }
            td {
                if let Some(completed_at) = dispatch.completed_at.clone() {
                    Time { value: completed_at }
                } else {
                    {empty()}
                }
            }
            td {
                if let Some(case) = dispatch.authorized_by {
                    Link { to: Route::SettlementReturnDetail { return_id: case.clone() },
                        TruncatedText { value: case, portal: true }
                    }
                } else {
                    {empty()}
                }
            }
        }
    }
}

#[component]
fn EventSection(events: Vec<SettlementReturnEvent>) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "settlement-return-events-title",
            h2 { id: "settlement-return-events-title", {t!("settlement-return-events-title")} }
            div { class: "admin-shell__table-wrap",
                table { class: "admin-shell__table",
                    aria_label: t!("settlement-return-events-title"),
                    thead {
                        tr {
                            th { scope: "col", {t!("settlement-return-field-version")} }
                            th { scope: "col", {t!("settlement-return-event-kind")} }
                            th { scope: "col", {t!("settlement-return-event-time")} }
                            th { scope: "col", {t!("settlement-return-event-actor")} }
                            th { scope: "col", {t!("settlement-return-event-details")} }
                        }
                    }
                    tbody {
                        for event in events {
                            tr { key: "{event.case_version}",
                                td { "{event.case_version}" }
                                td { {label(event_key, &event.event_kind)} }
                                td { Time { value: event.occurred_at.clone() } }
                                td {
                                    {event.actor_id.clone().unwrap_or_else(|| t!("settlement-return-event-worker"))}
                                }
                                td {
                                    TruncatedText {
                                        value: event_details_text(&event.details),
                                        size: "wide",
                                        portal: true,
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
