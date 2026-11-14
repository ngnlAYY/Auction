//! 玩家支持汇总的分区表格：出价、成交、义务、投递与通知。
//! 所有字段都来自 `auction-contracts` 的只读安全投影；页面不展示 payload、正文或凭据。

use auction_web_shared::{TableColumns, TruncatedText, format_credit_amount, t};
use dioxus::prelude::*;

use crate::api::{
    PlayerSupportBid, PlayerSupportBundle, PlayerSupportDeal, PlayerSupportNotification,
    PlayerSupportObligation,
};
use crate::surface::{admin_world_label, format_admin_time};

use super::super::Route;
use super::super::deliveries::helpers::{delivery_category_label, delivery_status_label};

/// 汇总头部：用户身份与截断提示。
pub(super) fn summary(bundle: &PlayerSupportBundle) -> Element {
    let user = bundle.user.clone();
    rsx! {
        section { class: "admin-panel admin-support-summary", aria_labelledby: "support-summary-title",
            h2 { id: "support-summary-title", {t!("support-summary-title")} }
            if let Some(user) = user {
                dl { class: "admin-support-summary__facts",
                    dt { {t!("support-user")} }
                    dd { TruncatedText { value: user.username.clone(), portal: true, legacy_dom: true } }
                    dt { {t!("support-user-id")} }
                    dd { TruncatedText { value: user.id.clone(), portal: true, legacy_dom: true } }
                    dt { {t!("support-user-status")} }
                    dd {
                        if user.status == "blacklisted" {
                            {t!("admin-users-status-blacklisted")}
                        } else {
                            {t!("admin-users-status-active")}
                        }
                    }
                    if let Some(identity) = user.game_player_id {
                        dt { {t!("support-user-game-id")} }
                        dd { TruncatedText { value: identity, portal: true, legacy_dom: true } }
                    }
                }
            } else {
                p { class: "admin-panel__note", {t!("support-no-user")} }
            }
            if bundle.truncated {
                p { class: "admin-panel__note", role: "status", {t!("support-truncated")} }
            }
            if !bundle.references.is_empty() {
                ul { class: "admin-support-references",
                    for reference in bundle.references.clone() {
                        li { "{reference.kind}: {reference.id}" }
                    }
                }
            }
        }
    }
}

pub(super) fn bids(bids: Vec<PlayerSupportBid>) -> Element {
    rsx! {
        section { class: "admin-panel admin-support-section", aria_labelledby: "support-bids-title",
            h2 { id: "support-bids-title", {t!("support-bids-title")} }
            if bids.is_empty() {
                p { class: "admin-panel__note", {t!("support-empty")} }
            } else {
                div { class: "admin-table-wrap",
                    table { class: "admin-table",
                        TableColumns { layout: "admin-support-bids", widths: vec![18, 10, 10, 12, 12, 18, 20] }
                        thead {
                            tr {
                                th { scope: "col", {t!("support-lot")} }
                                th { scope: "col", {t!("support-world")} }
                                th { scope: "col", {t!("support-lot-number")} }
                                th { scope: "col", {t!("support-amount")} }
                                th { scope: "col", {t!("support-mechanism")} }
                                th { scope: "col", {t!("delivery-status")} }
                                th { scope: "col", {t!("support-created-at")} }
                            }
                        }
                        tbody {
                            for bid in bids {
                                tr {
                                    td { Link { to: Route::AuctionLots { auction_id: bid.auction_id.clone() }, {bid.lot_id.clone()} } }
                                    td { {admin_world_label(&bid.world_id)} }
                                    td { "{bid.lot_number}" }
                                    td { {format_credit_amount(bid.amount)} }
                                    td { "{bid.mechanism}" }
                                    td { "{bid.lot_status}" }
                                    td { {format_admin_time(&bid.created_at)} }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn deals(deals: Vec<PlayerSupportDeal>) -> Element {
    rsx! {
        section { class: "admin-panel admin-support-section", aria_labelledby: "support-deals-title",
            h2 { id: "support-deals-title", {t!("support-deals-title")} }
            if deals.is_empty() {
                p { class: "admin-panel__note", {t!("support-empty")} }
            } else {
                div { class: "admin-table-wrap",
                    table { class: "admin-table",
                        TableColumns { layout: "admin-support-deals", widths: vec![18, 26, 10, 10, 14, 22] }
                        thead {
                            tr {
                                th { scope: "col", {t!("support-lot")} }
                                th { scope: "col", {t!("support-equipment")} }
                                th { scope: "col", {t!("support-world")} }
                                th { scope: "col", {t!("support-lot-number")} }
                                th { scope: "col", {t!("support-final-price")} }
                                th { scope: "col", {t!("support-ended-at")} }
                            }
                        }
                        tbody {
                            for deal in deals {
                                tr {
                                    td { Link { to: Route::AuctionLots { auction_id: deal.auction_id.clone() }, {deal.lot_id.clone()} } }
                                    td { TruncatedText { value: deal.equipment_name.clone(), portal: true, legacy_dom: true } }
                                    td { {admin_world_label(&deal.world_id)} }
                                    td { "{deal.lot_number}" }
                                    td { {format_credit_amount(deal.final_price)} }
                                    td {
                                        if let Some(at) = deal.effective_end_at {
                                            {format_admin_time(&at)}
                                        } else {
                                            {t!("value-empty")}
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
}

pub(super) fn obligations(obligations: Vec<PlayerSupportObligation>) -> Element {
    rsx! {
        section { class: "admin-panel admin-support-section", aria_labelledby: "support-obligations-title",
            h2 { id: "support-obligations-title", {t!("support-obligations-title")} }
            if obligations.is_empty() {
                p { class: "admin-panel__note", {t!("support-empty")} }
            } else {
                div { class: "admin-table-wrap",
                    table { class: "admin-table",
                        TableColumns { layout: "admin-support-obligations", widths: vec![18, 16, 10, 12, 12, 14, 18] }
                        thead {
                            tr {
                                th { scope: "col", {t!("support-obligation")} }
                                th { scope: "col", {t!("support-kind")} }
                                th { scope: "col", {t!("support-world")} }
                                th { scope: "col", {t!("support-amount")} }
                                th { scope: "col", {t!("support-business-state")} }
                                th { scope: "col", {t!("support-fulfillment")} }
                                th { scope: "col", {t!("support-delivery")} }
                            }
                        }
                        tbody {
                            for obligation in obligations {
                                tr {
                                    td { TruncatedText { value: obligation.obligation_id.clone(), portal: true, legacy_dom: true } }
                                    td { "{obligation.obligation_kind}" }
                                    td { {admin_world_label(&obligation.world_id)} }
                                    td { {format_credit_amount(obligation.amount)} }
                                    td { "{obligation.business_state}" }
                                    td {
                                        {obligation.fulfillment_state.clone().unwrap_or_else(|| t!("value-empty"))}
                                        if let Some(code) = obligation.fulfillment_error_code.clone() {
                                            span { class: "admin-panel__note", " ({code})" }
                                        }
                                    }
                                    td {
                                        if let Some(delivery_id) = obligation.delivery_id {
                                            Link { to: Route::DeliveryDetail { delivery_id }, {t!("support-open-delivery")} }
                                        } else {
                                            {t!("value-empty")}
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
}

pub(super) fn deliveries(deliveries: Vec<crate::api::Delivery>) -> Element {
    rsx! {
        section { class: "admin-panel admin-support-section", aria_labelledby: "support-deliveries-title",
            h2 { id: "support-deliveries-title", {t!("support-deliveries-title")} }
            if deliveries.is_empty() {
                p { class: "admin-panel__note", {t!("support-empty")} }
            } else {
                div { class: "admin-table-wrap",
                    table { class: "admin-table",
                        TableColumns { layout: "admin-support-deliveries", widths: vec![10, 20, 14, 12, 12, 12, 20] }
                        thead {
                            tr {
                                th { scope: "col", {t!("delivery-id")} }
                                th { scope: "col", {t!("delivery-to")} }
                                th { scope: "col", {t!("delivery-template")} }
                                th { scope: "col", {t!("delivery-category")} }
                                th { scope: "col", {t!("delivery-status")} }
                                th { scope: "col", {t!("delivery-outcome")} }
                                th { scope: "col", {t!("support-created-at")} }
                            }
                        }
                        tbody {
                            for delivery in deliveries {
                                tr {
                                    td {
                                        Link { to: Route::DeliveryDetail { delivery_id: delivery.id.clone() }, {delivery.id.chars().take(8).collect::<String>()} }
                                    }
                                    td { TruncatedText { value: delivery.recipient.clone(), portal: true, legacy_dom: true } }
                                    td {
                                        {delivery.template.clone().unwrap_or_else(|| t!("value-empty"))}
                                    }
                                    td {
                                        {delivery.category.as_deref().map_or_else(|| t!("value-empty"), delivery_category_label)}
                                    }
                                    td { {delivery_status_label(&delivery.status)} }
                                    td {
                                        if let Some(reason) = delivery.retirement_reason.clone() {
                                            {t!("delivery-outcome-retired", reason: reason)}
                                        } else if let Some(reason) = delivery.suppression_reason.clone() {
                                            {t!("delivery-outcome-suppressed", reason: reason)}
                                        } else {
                                            {delivery.failure_code.clone().unwrap_or_default()}
                                        }
                                    }
                                    td { {format_admin_time(&delivery.created_at)} }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn notifications(notifications: Vec<PlayerSupportNotification>) -> Element {
    rsx! {
        section { class: "admin-panel admin-support-section", aria_labelledby: "support-notifications-title",
            h2 { id: "support-notifications-title", {t!("support-notifications-title")} }
            if notifications.is_empty() {
                p { class: "admin-panel__note", {t!("support-empty")} }
            } else {
                div { class: "admin-table-wrap",
                    table { class: "admin-table",
                        TableColumns { layout: "admin-support-notifications", widths: vec![18, 10, 14, 12, 12, 18, 16] }
                        thead {
                            tr {
                                th { scope: "col", {t!("support-notification")} }
                                th { scope: "col", {t!("support-channel")} }
                                th { scope: "col", {t!("support-kind")} }
                                th { scope: "col", {t!("delivery-status")} }
                                th { scope: "col", {t!("delivery-outcome")} }
                                th { scope: "col", {t!("support-delivery")} }
                                th { scope: "col", {t!("support-created-at")} }
                            }
                        }
                        tbody {
                            for notification in notifications {
                                tr {
                                    td { TruncatedText { value: notification.notification_id.clone(), portal: true, legacy_dom: true } }
                                    td { "{notification.channel}" }
                                    td {
                                        {notification.kind.clone().unwrap_or_else(|| t!("value-empty"))}
                                    }
                                    td { "{notification.state}" }
                                    td {
                                        {notification.state_reason.clone().unwrap_or_default()}
                                    }
                                    td {
                                        if let Some(delivery_id) = notification.delivery_id.clone() {
                                            Link { to: Route::DeliveryDetail { delivery_id }, {t!("support-open-delivery")} }
                                        } else {
                                            {t!("value-empty")}
                                        }
                                    }
                                    td { {format_admin_time(&notification.created_at)} }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
