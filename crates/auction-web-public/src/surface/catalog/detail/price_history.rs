//! 拍品详情抽屉的价格历史弹窗：按装备名列出当前世界往期拍卖的成交价。
//! 只读取既有的公开价格历史接口，不新增筛选语义。名称查询是“各词都包含”的匹配，
//! 结果可能含名称更长的同类装备，这类行会标出实际装备名。

use auction_web_shared::{
    Dialog, TableColumns, Time, format_credit_amount as format_amount, legacy_t as t,
};
use dioxus::prelude::*;

use crate::api::{
    PriceHistoryFetchError, PriceHistoryPage, PriceHistoryQuery, fetch_price_history,
};
use crate::surface::PublicPager;
use crate::surface::reads::history_equipment_name_query;

/// 弹窗每页条数；页码只存在于弹窗内，不写地址栏。
const PRICE_HISTORY_PAGE_SIZE: i64 = 10;

/// 同名装备的往期成交价弹窗。与完整竞价历史弹窗一样作为抽屉的兄弟节点渲染，关闭后焦点
/// 回到抽屉里打开它的按钮。成交记录不随实时事件变化，这里不订阅出价刷新。
#[component]
pub(super) fn DetailPriceHistoryDialog(
    lot_code: String,
    lot_name: String,
    on_close: EventHandler<()>,
) -> Element {
    let selected_world = use_context::<Signal<String>>();
    let mut page = use_signal(|| 1_i64);
    let history = use_resource(use_reactive(&lot_name, move |lot_name| {
        let world = selected_world();
        let requested = page();
        async move {
            // 名称得不到可用查询词时不发请求，按没有成交记录展示。
            let Some(name) = history_equipment_name_query(&lot_name) else {
                return Ok(None);
            };
            let query = PriceHistoryQuery {
                world: Some(world),
                name: Some(name),
                page: requested,
                ..PriceHistoryQuery::default()
            };
            fetch_price_history(query, PRICE_HISTORY_PAGE_SIZE).await.map(Some)
        }
    }));
    let snapshot = history.read_unchecked();

    rsx! {
        Dialog {
            title: t!("nav-price-history"),
            close_label: t!("catalog-close"),
            on_close: move |_| on_close.call(()),
            div { class: "public-bid-dialog__lot public-catalog-history__lot",
                div { class: "public-bid-dialog__lot-name",
                    span { class: "public-bid-dialog__lot-code", "{lot_code}" }
                    span { class: "public-bid-dialog__lot-title", "{lot_name}" }
                }
            }
            div { class: "public-catalog-history",
                p { class: "public-catalog-history__note", {t!("catalog-price-history-note")} }
                // 翻页期间保留上一页的表格，新页到达后整体替换；只有首次读取时正文为空。
                {match snapshot.as_ref() {
                    None => rsx! {},
                    Some(Err(PriceHistoryFetchError::Unsupported)) => rsx! {
                        p { class: "public-catalog-detail__empty", {t!("page-history-unsupported")} }
                    },
                    Some(Err(PriceHistoryFetchError::Unavailable)) => rsx! {
                        p { class: "public-catalog-detail__status", role: "alert", {t!("data-error")} }
                    },
                    Some(Ok(Some(history))) if !history.rows.is_empty() => rsx! {
                        PriceHistoryRows { page: history.clone(), lot_name: lot_name.clone() }
                        PublicPager {
                            page: history.page.unwrap_or(1),
                            page_size: PRICE_HISTORY_PAGE_SIZE,
                            total: history.total,
                            has_more: history.has_more.unwrap_or(false),
                            hide_when_single: true,
                            show_size_picker: false,
                            label: t!("nav-price-history"),
                            on_page_change: move |next| page.set(next),
                            on_page_clamp: move |next| page.set(next),
                        }
                    },
                    Some(Ok(_)) => rsx! {
                        p { class: "public-catalog-detail__empty", {t!("catalog-price-history-empty")} }
                    },
                }}
            }
        }
    }
}

/// 成交价表格。名称与当前拍品不同（忽略大小写）的行在期数下方标出实际装备名。
#[component]
fn PriceHistoryRows(page: PriceHistoryPage, lot_name: String) -> Element {
    rsx! {
        div { class: "public-auction-lot__table",
            table {
                caption { class: "public-visually-hidden", {t!("nav-price-history")} }
                TableColumns { layout: "public-lot-price-history", widths: vec![38, 14, 20, 28] }
                thead {
                    tr {
                        th { {t!("history-round")} }
                        th { {t!("history-level")} }
                        th { class: "public-auction-lot__number", {t!("history-price")} }
                        th { {t!("history-closed")} }
                    }
                }
                tbody {
                    for row in page.rows {
                        tr { key: "{row.lot_id}",
                            td { "data-label": t!("history-round"),
                                span { "#{row.auction_sequence}" }
                                if !row.equipment_name.trim().eq_ignore_ascii_case(lot_name.trim()) {
                                    span { class: "public-catalog-history__variant",
                                        "{row.equipment_name}"
                                    }
                                }
                            }
                            td { "data-label": t!("history-level"),
                                {row.equipment_level.map_or_else(|| t!("value-empty"), |level| level.to_string())}
                            }
                            td {
                                class: "public-auction-lot__amount public-auction-lot__number",
                                "data-label": t!("history-price"),
                                "{format_amount(row.price)}"
                            }
                            td { "data-label": t!("history-closed"), Time { value: row.closed_at } }
                        }
                    }
                }
            }
        }
    }
}
