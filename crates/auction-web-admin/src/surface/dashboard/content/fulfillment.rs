//! 履约健康面板：积压状态、分项计数、已寄出等待领取分组、长期未领取清单与需要人工介入的提示。

use auction_web_shared::{Time, t};
use dioxus::prelude::*;

use super::derived::DashboardDerived;
use super::widgets::{badge, card, semantic_card, text};
use crate::api::{AdminCollectionOverdueItem, AdminDashboardSnapshot};
use crate::surface::admin_world_label;
use crate::surface::lots::admin_lot_code_for;
use crate::surface::router::Route;

/// 等待领取按整天展示寄出后的等待时长（向下取整）。
const SECONDS_PER_DAY: i64 = 86_400;

pub(super) fn fulfillment_panel(
    snapshot: &AdminDashboardSnapshot,
    derived: &DashboardDerived,
) -> Element {
    let fulfillment = snapshot.fulfillment.data.as_ref();
    let fulfillment_state = derived.fulfillment_state;
    let fulfillment_backlog = derived.fulfillment_backlog;
    rsx! {
    section { class: "admin-dashboard-panel", aria_labelledby: "dashboard-fulfillment-title",
    h2 { id: "dashboard-fulfillment-title", {t!("dashboard-legacy-fulfillment-health")} }
    if let Some(fulfillment) = fulfillment {
        div { class: "admin-dashboard-cards",
            {semantic_card(
                "dashboard-fulfillment-state",
                t!("dashboard-legacy-fulfillment-health"),
                fulfillment_state.to_owned(),
                badge(fulfillment_state, fulfillment_status(fulfillment_state)),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-pending"),
                badge(
                    if fulfillment.pending > 0 { "warn" } else { "ok" },
                    fulfillment.pending.to_string(),
                ),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-blocked"),
                badge(
                    if fulfillment.blocked > 0 { "error" } else { "ok" },
                    fulfillment.blocked.to_string(),
                ),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-failed"),
                badge(
                    if fulfillment.failed > 0 { "error" } else { "ok" },
                    fulfillment.failed.to_string(),
                ),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-manual-reconcile"),
                badge(
                    if fulfillment.manual_reconcile > 0 { "error" } else { "ok" },
                    fulfillment.manual_reconcile.to_string(),
                ),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-waiting-payment"),
                text(fulfillment.waiting_payment.to_string()),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-waiting-prerequisite"),
                text(fulfillment.waiting_prerequisite.to_string()),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-oldest-backlog"),
                text(t!(
                    "dashboard-legacy-duration-seconds",
                    seconds: fulfillment.oldest_backlog_age_seconds
                )),
            )}
        }
        // 已寄出、只等收件人领取的义务单列：不计入待处理，但“读不到”与“未领取”分开显示。
        // 主计数是寄出时长达到长期未领取阈值的件数，总等待数只作背景。
        h3 { {t!("dashboard-fulfillment-awaiting-title")} }
        div { class: "admin-dashboard-cards",
            {card(
                t!("dashboard-fulfillment-awaiting-overdue"),
                badge(
                    if fulfillment.collection_overdue_count > 0 { "warn" } else { "ok" },
                    fulfillment.collection_overdue_count.to_string(),
                ),
            )}
            {card(
                t!("dashboard-fulfillment-awaiting-total"),
                text(fulfillment.awaiting_collection.to_string()),
            )}
            {card(
                t!("dashboard-fulfillment-awaiting-oldest"),
                text(t!(
                    "dashboard-fulfillment-awaiting-days",
                    days: fulfillment.oldest_awaiting_collection_age_seconds / SECONDS_PER_DAY
                )),
            )}
            {card(
                t!("dashboard-fulfillment-awaiting-unreadable"),
                badge(
                    if fulfillment.awaiting_collection_unreadable > 0 { "warn" } else { "ok" },
                    fulfillment.awaiting_collection_unreadable.to_string(),
                ),
            )}
        }
        p { class: "admin-panel__note", {t!("dashboard-fulfillment-awaiting-note")} }
        if !fulfillment.collection_overdue_items.is_empty() {
            {overdue_list(&fulfillment.collection_overdue_items, fulfillment.collection_overdue_count)}
        }
        if fulfillment_backlog {
            aside {
                class: "admin-dashboard-attention",
                aria_labelledby: "dashboard-fulfillment-attention-title",
                "data-state": "error",
                div {
                    class: "admin-dashboard-attention__copy",
                    role: "status",
                    aria_atomic: "true",
                    h3 {
                        id: "dashboard-fulfillment-attention-title",
                        {t!("dashboard-fulfillment-attention-title")}
                    }
                    p { {t!("dashboard-fulfillment-attention-note")} }
                }
                nav {
                    class: "admin-dashboard-attention__actions",
                    aria_label: t!("dashboard-fulfillment-attention-title"),
                    Link { to: Route::Deliveries {}, {t!("dashboard-open-deliveries")} }
                    Link { to: Route::Settlement {}, {t!("nav-settlement")} }
                }
            }
        }
    } else {
        p { class: "admin-dashboard-status",
            {t!("dashboard-legacy-fulfillment-status-unavailable")}
        }
    }

    }
    }
}

/// 长期未领取清单：只列寄出时长达到阈值的等待领取，寄出最早优先；条数有上限，总超期件数在
/// 说明里给出。只读展示，不提供重发或撤回入口。
fn overdue_list(items: &[AdminCollectionOverdueItem], overdue_count: i64) -> Element {
    rsx! {
        h3 { id: "dashboard-fulfillment-overdue-title", {t!("dashboard-fulfillment-overdue-title")} }
        p { class: "admin-panel__note",
            {t!("dashboard-fulfillment-overdue-summary", count: overdue_count, shown: items.len())}
        }
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table",
                aria_labelledby: "dashboard-fulfillment-overdue-title",
                thead {
                    tr {
                        th { scope: "col", {t!("dashboard-fulfillment-overdue-lot")} }
                        th { scope: "col", {t!("dashboard-fulfillment-overdue-equipment")} }
                        th { scope: "col", {t!("dashboard-fulfillment-overdue-kind")} }
                        th { scope: "col", {t!("dashboard-fulfillment-overdue-recipient")} }
                        th { scope: "col", {t!("dashboard-fulfillment-overdue-sent-at")} }
                        th { scope: "col", {t!("dashboard-fulfillment-overdue-waited")} }
                        th { scope: "col", {t!("dashboard-fulfillment-overdue-read")} }
                    }
                }
                tbody {
                    for item in items.iter() {
                        tr { key: "{item.obligation_id}",
                            td {
                                {t!(
                                    "admin-auctions-row-heading",
                                    world: admin_world_label(&item.world),
                                    number: item.auction_sequence
                                )}
                                " "
                                {overdue_lot_code(item)}
                            }
                            td { "{item.equipment_name}" }
                            td { {overdue_kind_label(&item.obligation_kind)} }
                            td { "{item.recipient}" }
                            td { Time { value: item.sent_at.clone() } }
                            td {
                                {t!(
                                    "dashboard-fulfillment-awaiting-days",
                                    days: item.age_seconds / SECONDS_PER_DAY
                                )}
                            }
                            td {
                                // 没有读取失败记录只说明最近一次读取没有失败，不是“已读到、
                                // 确认未领取”的证据。
                                if item.last_read_failed {
                                    {badge("warn", t!("dashboard-fulfillment-overdue-read-failed"))}
                                } else {
                                    {t!("dashboard-fulfillment-overdue-read-no-failure")}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 清单项的拍品代码：分类前缀加分类内编号（服务端 `lot_number` 即 `lots.category_number`），
/// 与 Admin lot 页、结算报表一致。
fn overdue_lot_code(item: &AdminCollectionOverdueItem) -> String {
    admin_lot_code_for(&item.equipment_category, item.lot_number)
}

/// 等待领取义务类别的当前语言名称；解码已拒绝闭集外的值，这里只兜底原样显示。
fn overdue_kind_label(kind: &str) -> String {
    match kind {
        "seller_payout" => t!("dashboard-fulfillment-overdue-kind-seller-payout"),
        "buyer_refund" => t!("dashboard-fulfillment-overdue-kind-buyer-refund"),
        "unsold_return" => t!("dashboard-fulfillment-overdue-kind-unsold-return"),
        "withdrawn_return" => t!("dashboard-fulfillment-overdue-kind-withdrawn-return"),
        "winner_equipment_handoff" => t!("dashboard-fulfillment-overdue-kind-handoff"),
        _ => kind.to_owned(),
    }
}

fn fulfillment_status(state: &str) -> String {
    match state {
        "ok" => t!("dashboard-legacy-fulfillment-status-ok"),
        "error" => t!("dashboard-legacy-fulfillment-status-degraded"),
        _ => t!("dashboard-legacy-fulfillment-status-unavailable"),
    }
}

#[cfg(test)]
mod tests {
    use super::{AdminCollectionOverdueItem, overdue_lot_code};

    /// 混合类别的场次里全场第 100 件、盾牌分类第 2 件：服务端给出分类内编号 2，代码是 `Shd02`。
    #[test]
    fn overdue_lot_code_uses_the_category_number() {
        let item = AdminCollectionOverdueItem {
            obligation_id: "00000000-0000-0000-0000-000000000010".to_owned(),
            obligation_kind: "seller_payout".to_owned(),
            world: "main".to_owned(),
            auction_sequence: 4,
            lot_number: 2,
            equipment_category: "Shield".to_owned(),
            equipment_name: "Radiant Buckler".to_owned(),
            recipient: "collection-seller".to_owned(),
            sent_at: "2026-01-01T00:00:00.000000Z".to_owned(),
            age_seconds: 691_200,
            last_read_failed: false,
        };
        assert_eq!(overdue_lot_code(&item), "Shd02");
    }
}
