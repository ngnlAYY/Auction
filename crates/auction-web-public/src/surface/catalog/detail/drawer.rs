//! 目录拍品详情抽屉：右侧滑出，展示所选行的实时投影、装备属性与最近出价。
//! 抽屉不重算出价资格、不创建 `command_id`：出价入口照行内操作栏写入的投影渲染，
//! 点击后交给页面级命令弹窗。

use std::collections::BTreeMap;

use auction_contracts::MAX_PUBLIC_BID_AMOUNT;
use auction_web_shared::{format_credit_amount as format_amount, legacy_t as t};
use dioxus::prelude::*;

use crate::api::{LotDetailItem, fetch_lot_detail};
use crate::surface::catalog::command_scope::CommandDialogContext;
use crate::surface::watches::LotWatchButton;
use crate::surface::{
    catalog_category_key, catalog_category_label, countdown_class, equipment_group_label,
    is_activation_key, login_href_for_current_location, lot_status_class, lot_status_label,
    use_countdown_display,
};

use super::bid_history::{DetailBidHistoryDialog, DetailLatestBids};
use super::price_history::DetailPriceHistoryDialog;
use super::{CatalogDetailBid, CatalogDetailDialogContext, CatalogDetailInput, DetailWorld};

/// 属性默认展示的条数；超过时提供“显示全部（N）”。
const COLLAPSED_STATS: usize = 6;
const TITLE_ID: &str = "public-catalog-detail-title";
const BID_REASON_ID: &str = "public-catalog-detail-bid-reason";

/// 抽屉之上再开弹窗（出价、完整竞价历史、价格历史）时记下入口。弹窗关闭后焦点陷阱把焦点交给
/// 抽屉内带 autofocus 的控件，于是回到打开它的按钮；刚打开抽屉时落在关闭按钮。
#[derive(Clone, Copy, PartialEq)]
enum DetailFocus {
    Close,
    Bid,
    History,
    PriceHistory,
}

/// 详情抽屉宿主。挂在拍卖作用域内（标记上下文之下），拍卖切换即卸载并关闭抽屉。
#[component]
pub(crate) fn CatalogDetailHost(auction_id: String) -> Element {
    let mut detail = use_context::<CatalogDetailDialogContext>().0;
    use_drop(move || {
        // 身份宿主先于本宿主销毁时信号已不可写，直接跳过。
        if let Ok(mut current) = detail.try_write() {
            *current = None;
        }
    });
    let current = detail.read().clone();

    rsx! {
        // 以拍品为 key 的单元素列表：换拍品时整体重建抽屉，属性展开、历史弹窗与读取任务不串到下一件。
        for (lot_id, input) in current.into_iter().map(|input| (input.row.lot_id.clone(), input)) {
            CatalogDetailDrawer {
                key: "{lot_id}",
                auction_id: auction_id.clone(),
                input,
                world: DetailWorld::Selected,
                on_close: move |_| detail.set(None),
            }
        }
    }
}

/// 抽屉本体。展示状态归调用方所有：目录宿主清空页面级信号，只读入口清空自己的查找状态。
#[component]
pub(super) fn CatalogDetailDrawer(
    auction_id: String,
    input: CatalogDetailInput,
    /// 价格历史按哪个世界查询；见 [`DetailWorld`]。
    world: DetailWorld,
    on_close: EventHandler<()>,
) -> Element {
    let CatalogDetailInput { row, bid, watchable } = input;
    // 只读入口不在命令宿主之下，此时出价入口必为 `Unavailable`，命令弹窗信号不会被用到。
    let command_dialog = try_use_context::<CommandDialogContext>().map(|context| context.0);
    let mut focus = use_signal(|| DetailFocus::Close);
    let mut history_open = use_signal(|| false);
    let mut price_history_open = use_signal(|| false);
    let close = use_callback(move |_: ()| on_close.call(()));

    let is_pending = row.status == "pending";
    let is_live = row.status == "live";
    let title = row.name.clone();
    // 原生 title 属性与标题文本是两个独立值；rsx 中属性与子节点求值顺序不保证。
    let title_attr = title.clone();
    let category = catalog_category_label(catalog_category_key(&row.category));
    let group = equipment_group_label(&row.equipment_group);
    let status = lot_status_label(&row.status);
    let status_class = lot_status_class(&row.status);
    let start_price = format_amount(row.start_price);
    let current_price = format_amount(row.current_price);
    let next_bid = if row.next_bid > MAX_PUBLIC_BID_AMOUNT {
        t!("bid-limit-reached")
    } else {
        format_amount(row.next_bid)
    };
    let leader = row.leader_name.clone().unwrap_or_else(|| t!("value-empty"));
    let primary = row.primary_attribute_initials.clone().unwrap_or_else(|| t!("value-empty"));
    let avg_roll = row.avg_roll.map(|value| format!("{value:.1}%"));
    let level = row.level.map(|value| value.to_string());
    let remaining = if is_live { row.remaining.clone() } else { String::new() };
    let game_url = row.game_equipment_url.clone();

    // 出价入口三态与行内操作栏一致：命令按钮（可禁用并说明原因）、登录链接或不渲染。
    let bid_reason = match &bid {
        CatalogDetailBid::Command { input, disabled: true } => input.blocked_reason.clone(),
        _ => String::new(),
    };
    let bid_entry = match bid {
        CatalogDetailBid::Command { input, disabled } => rsx! {
            button {
                class: "public-catalog-detail__bid",
                r#type: "button",
                aria_haspopup: "dialog",
                disabled,
                autofocus: focus() == DetailFocus::Bid,
                aria_describedby: (!bid_reason.is_empty()).then_some(BID_REASON_ID),
                onclick: move |_| {
                    // 已打开的模态命令不接受另一个入口替换负载或目标；没有命令宿主时失败关闭。
                    if let Some(mut command_dialog) = command_dialog
                        && command_dialog.peek().is_none()
                    {
                        focus.set(DetailFocus::Bid);
                        command_dialog.set(Some((*input).clone()));
                    }
                },
                {t!("catalog-bid")}
            }
        },
        CatalogDetailBid::Login => rsx! {
            Link {
                class: "public-catalog-detail__bid",
                to: login_href_for_current_location(),
                {t!("catalog-login-to-bid")}
            }
        },
        CatalogDetailBid::Unavailable => rsx! {},
    };

    rsx! {
        // 抽屉没有未提交的输入，点遮罩即关闭；关闭会同步卸载抽屉，阻止事件继续冒泡到已丢弃的处理器。
        div {
            class: "public-catalog-detail__scrim",
            onclick: move |event| {
                event.stop_propagation();
                close.call(());
            },
        }
        aside {
            class: "public-catalog-detail",
            role: "dialog",
            aria_modal: "true",
            aria_labelledby: TITLE_ID,
            tabindex: "-1",
            onkeydown: move |event| {
                if event.key() == Key::Escape {
                    event.prevent_default();
                    event.stop_propagation();
                    close.call(());
                } else if event.is_auto_repeating() && is_activation_key(&event.key()) {
                    // 在目录行上按住 Enter/Space 打开抽屉后，重复按键落到刚获得焦点的关闭按钮上；
                    // 不拦住的话抽屉会被同一次按键立刻关掉。
                    event.prevent_default();
                }
            },
            header { class: "public-catalog-detail__head",
                div { id: TITLE_ID, class: "public-catalog-detail__heading",
                    div { class: "public-catalog-detail__eyebrow",
                        span { {t!("catalog-detail")} }
                        span { class: "public-catalog-detail__code", "{row.code}" }
                    }
                    h2 { class: "public-catalog-detail__title", title: title_attr, "{title}" }
                }
                button {
                    class: "public-catalog-detail__close",
                    r#type: "button",
                    aria_label: t!("catalog-close"),
                    title: t!("catalog-close"),
                    autofocus: focus() == DetailFocus::Close,
                    onclick: move |event| {
                        event.stop_propagation();
                        close.call(());
                    },
                    DetailCloseIcon {}
                }
            }
            div { class: "public-catalog-detail__body",
                dl { class: "public-catalog-detail__facts",
                    div { dt { {t!("equipment-group")} } dd { "{group}" } }
                    div { dt { {t!("equipment-category")} } dd { "{category}" } }
                    if let Some(level) = level {
                        div { dt { {t!("equipment-level")} } dd { "{level}" } }
                    }
                    div { dt { {t!("catalog-primary-attributes")} } dd { "{primary}" } }
                    if let Some(avg_roll) = avg_roll {
                        div { dt { {t!("equipment-avg-roll")} } dd { "{avg_roll}" } }
                    }
                    div { dt { {t!("catalog-seller")} } dd { "{row.seller_name}" } }
                    if is_pending {
                        // 预览阶段没有出价；目录投影里的当前价与下一口价都等于起拍价。
                        div { dt { {t!("catalog-start-price")} } dd { "{start_price}" } }
                    } else {
                        div { dt { {t!("catalog-leader")} } dd { "{leader}" } }
                        div { dt { {t!("catalog-current")} } dd { "{current_price}" } }
                    }
                    if is_live {
                        div { dt { {t!("catalog-next-bid")} } dd { "{next_bid}" } }
                    }
                    if !remaining.is_empty() {
                        div { dt { {t!("catalog-remaining")} } dd { DetailCountdown { ends_at: remaining } } }
                    }
                    div { dt { {t!("catalog-status")} }
                        dd {
                            span { class: "public-auction-status public-auction-status--{status_class}", "{status}" }
                        }
                    }
                }
                DetailStats { lot_id: row.lot_id.clone() }
                if !is_pending {
                    DetailLatestBids {
                        auction_id: auction_id.clone(),
                        lot_id: row.lot_id.clone(),
                        return_focus: focus() == DetailFocus::History,
                        on_view_all: move |_| {
                            focus.set(DetailFocus::History);
                            history_open.set(true);
                        },
                    }
                }
            }
            footer { class: "public-catalog-detail__foot",
                if !bid_reason.is_empty() {
                    p { id: BID_REASON_ID, class: "public-catalog-detail__reason", role: "note", "{bid_reason}" }
                }
                div { class: "public-catalog-detail__foot-actions",
                    {bid_entry}
                    if watchable {
                        LotWatchButton {
                            lot_id: row.lot_id.clone(),
                            lot_code: row.code.clone(),
                            icon_only: false,
                        }
                    }
                    button {
                        class: "public-catalog-detail__price-history",
                        r#type: "button",
                        aria_haspopup: "dialog",
                        autofocus: focus() == DetailFocus::PriceHistory,
                        onclick: move |_| {
                            focus.set(DetailFocus::PriceHistory);
                            price_history_open.set(true);
                        },
                        {t!("nav-price-history")}
                    }
                    if !game_url.is_empty() {
                        a {
                            class: "public-catalog-detail__game-link",
                            href: game_url,
                            target: "_blank",
                            rel: "noopener noreferrer",
                            {t!("equipment-view-in-game")}
                        }
                    }
                }
            }
        }
        if history_open() {
            DetailBidHistoryDialog {
                auction_id: auction_id.clone(),
                lot_id: row.lot_id.clone(),
                lot_code: row.code.clone(),
                lot_name: row.name.clone(),
                on_close: move |_| history_open.set(false),
            }
        }
        if price_history_open() {
            DetailPriceHistoryDialog {
                auction_id,
                lot_code: row.code.clone(),
                lot_name: row.name.clone(),
                world,
                on_close: move |_| price_history_open.set(false),
            }
        }
    }
}

/// 装备属性。目录行不带属性，打开抽屉时按拍品读取一次（经公开读缓存）；
/// 读取中保留区块高度而不放“加载中”文字，失败时提供重试。
#[component]
fn DetailStats(lot_id: String) -> Element {
    let mut expanded = use_signal(|| false);
    let mut stats = use_resource(use_reactive((&lot_id,), move |(lot_id,)| async move {
        fetch_lot_detail(lot_id).await.map(|page| match page.lot {
            Some(LotDetailItem::Live(lot)) => stat_rows(lot.equipment.stats),
            Some(LotDetailItem::Preview(lot)) => stat_rows(lot.equipment.stats),
            None => Vec::new(),
        })
    }));
    let snapshot = stats.read_unchecked();
    let total = match snapshot.as_ref() {
        Some(Ok(rows)) => rows.len(),
        _ => 0,
    };
    // 装备没有任何属性时整块不渲染，不留空标题。
    if matches!(snapshot.as_ref(), Some(Ok(rows)) if rows.is_empty()) {
        return rsx! {};
    }

    rsx! {
        section { class: "public-catalog-detail__section",
            div { class: "public-catalog-detail__section-head",
                h3 { class: "public-catalog-detail__section-title", {t!("equipment-stats")} }
                if total > COLLAPSED_STATS {
                    button {
                        class: "public-catalog-detail__more",
                        r#type: "button",
                        aria_expanded: expanded().to_string(),
                        onclick: move |_| expanded.set(!expanded()),
                        if expanded() {
                            {t!("catalog-detail-show-less")}
                        } else {
                            {t!("catalog-detail-show-all", count: total.to_string())}
                        }
                    }
                }
            }
            {match snapshot.as_ref() {
                None => rsx! {
                    div {
                        class: "public-catalog-detail__stats public-catalog-detail__stats--pending",
                        aria_busy: "true",
                    }
                },
                Some(Err(_)) => rsx! {
                    p { class: "public-catalog-detail__status", role: "alert",
                        span { {t!("data-error")} }
                        button {
                            class: "public-catalog-detail__more",
                            r#type: "button",
                            onclick: move |_| stats.restart(),
                            {t!("data-retry")}
                        }
                    }
                },
                Some(Ok(rows)) => rsx! {
                    dl { class: "public-catalog-detail__stats",
                        for (name, value) in rows.iter().take(if expanded() { usize::MAX } else { COLLAPSED_STATS }) {
                            div { key: "{name}", class: "public-catalog-detail__stat",
                                dt { "{name}" }
                                dd { "{value}" }
                            }
                        }
                    }
                },
            }}
        }
    }
}

fn stat_rows(
    stats: BTreeMap<String, auction_contracts::PublicEquipmentStatResponse>,
) -> Vec<(String, String)> {
    stats.into_iter().map(|(name, stat)| (name, stat.value)).collect()
}

/// 剩余时间只订阅在抽屉叶子上，抽屉事实行不随秒针重渲染。
#[component]
fn DetailCountdown(ends_at: String) -> Element {
    let countdown = use_countdown_display(ends_at);
    rsx! {
        span { class: countdown_class(&countdown), "{countdown.label}" }
    }
}

#[component]
fn DetailCloseIcon() -> Element {
    rsx! {
        svg {
            class: "public-catalog-detail__close-icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.5",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: "M6 6l12 12M18 6 6 18" }
        }
    }
}
