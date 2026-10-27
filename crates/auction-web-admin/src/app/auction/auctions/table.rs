//! Admin 拍卖列表表格与筛选器。

use std::cmp::Ordering;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::app::surface::pagination::{AdminPager as Pager, AdminPagination};
use crate::app::surface::pending::PendingCommands;
use auction_web_shared::{AsyncState, AsyncStateKind};

use crate::api::{AdminAuction, AdminAuctionPage};

use super::super::intake::AdminIntakeForm;
use super::super::query::AuctionsQuery;
use super::super::sort::{SortDirection, SortableTh, directed};
use super::actions::{AdminAuctionNotice, AdminAuctionRow, auction_notice_label};
use super::create::AdminAuctionCreateForm;
use super::forum::AdminAuctionForumStatus;

#[component]
pub(super) fn AdminAuctionTable(
    state: Option<Result<AdminAuctionPage, String>>,
    pagination: AdminPagination,
    /// 当前世界筛选（地址栏恢复后的值）；变更经 `on_query_change` 由页面写回 URL。
    world: String,
    /// 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `on_sort_change` 由页面写回 URL。
    sort: String,
    direction: SortDirection,
    on_query_change: EventHandler<AuctionsQuery>,
    on_sort_change: EventHandler<(String, SortDirection)>,
    reload_generation: Signal<u64>,
) -> Element {
    // 秒级时钟订阅已下沉到行内的就绪单元格与动作单元格（见 row.rs/dialog.rs）：
    // 整表（筛选、表头、分页）不再随秒针重渲染，只有真正依赖当前时间的叶子组件每秒重算。
    let notice = use_signal(|| None::<AdminAuctionNotice>);
    let owner = use_context::<PendingCommands>();
    let intake_busy = owner.is_busy(&format!("intake:{world}"));
    // 翻页保留当前世界筛选与排序；该闭包在分支内move走一份克隆。
    let pager_world = world.clone();
    let pager_sort = sort.clone();
    let list_content = match state {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-auctions-loading"),
                detail: "",
            }
        },
        Some(Err(detail)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: detail,
                detail: "",
            }
        },
        Some(Ok(page)) => {
            let is_empty = page.auctions.is_empty();
            let capabilities = page.capabilities;
            let page_info = page.pagination;
            let mut auctions = page.auctions.clone();
            sort_auctions(&mut auctions, &sort, direction);
            rsx! {
                if capabilities.manual_intake && matches!(world.as_str(), "main" | "isekai") {
                    for world_id in [world.clone()] {
                        AdminIntakeForm {
                            key: "{world_id}",
                            world_id,
                        }
                    }
                }
                if !is_empty {
                    AdminAuctionForumStatus { auctions: page.auctions }
                }
                div { class: "admin-auction-table-wrap",
                    table { class: "admin-auction-table",
                        caption { class: "admin-visually-hidden", {t!("page-auctions-title")} }
                        auction_web_shared::TableColumns { layout: "admin-auctions", widths: vec![6, 6, 8, 18, 18, 15, 29] }
                        thead {
                            tr {
                                SortableTh {
                                    column: "sequence",
                                    label: t!("admin-auctions-sequence"),
                                    active: sort.clone(),
                                    direction,
                                    on_sort: on_sort_change,
                                    class: "admin-auction-col-number",
                                }
                                th { scope: "col", class: "admin-auction-col-world", {t!("admin-auctions-world")} }
                                SortableTh {
                                    column: "status",
                                    label: t!("admin-auctions-status"),
                                    active: sort.clone(),
                                    direction,
                                    on_sort: on_sort_change,
                                    class: "admin-auction-col-status",
                                }
                                SortableTh {
                                    column: "open_at",
                                    label: t!("admin-auctions-open-at"),
                                    active: sort.clone(),
                                    direction,
                                    on_sort: on_sort_change,
                                    class: "admin-auction-col-opens",
                                }
                                SortableTh {
                                    column: "end_at",
                                    label: t!("admin-auctions-end-at"),
                                    active: sort.clone(),
                                    direction,
                                    on_sort: on_sort_change,
                                    class: "admin-auction-col-ends",
                                }
                                th { scope: "col", class: "admin-auction-col-ready", {t!("admin-auctions-readiness")} }
                                th { scope: "col", {t!("admin-auctions-actions")} }
                            }
                        }
                        tbody {
                            for auction in auctions {
                                // 按业务 ID 固定 key，避免刷新/重排后行内本地状态错位绑定到另一行。
                                AdminAuctionRow {
                                    key: "{auction.id}",
                                    auction,
                                    capabilities: capabilities.clone(),
                                    notice,
                                }
                            }
                        }
                    }
                }
                if is_empty {
                    AsyncState {
                        kind: AsyncStateKind::Empty,
                        title: t!("admin-auctions-empty"),
                        detail: "",
                    }
                }
                Pager {
                    pagination,
                    page: page_info.page,
                    page_size: page_info.page_size,
                    total: Some(page_info.total),
                    label: t!("admin-auctions-pagination"),
                    previous_label: t!("admin-pagination-previous"),
                    status: move |(page, pages): (i64, i64)| {
                        t!("admin-pagination-status", page: page, pages: pages)
                    },
                    next_label: t!("admin-pagination-next"),
                    jump_label: t!("admin-pagination-input"),
                    jump_button_label: t!("admin-pagination-go"),
                    on_page_change: move |next_page| {
                        on_query_change
                            .call(AuctionsQuery {
                                world: pager_world.clone(),
                                page: next_page,
                                sort: pager_sort.clone(),
                                direction,
                            });
                    },
                }
            }
        }
    };

    rsx! {
        section { class: "admin-auction-page", aria_labelledby: "auction-title",
            h1 { id: "auction-title", {t!("page-auctions-title")} }
            div { class: "admin-auction-filter",
                form {
                    onsubmit: move |event| event.prevent_default(),
                    label {
                        r#for: "auction-world",
                        {t!("admin-auctions-world")}
                        select {
                            class: "admin-auction-world",
                            id: "auction-world",
                            disabled: intake_busy,
                            value: world,
                            onchange: move |event| {
                                if intake_busy { return; }
                                // 切换世界筛选回到第一页并保留排序，由页面 push 到地址栏。
                                on_query_change
                                    .call(AuctionsQuery {
                                        world: event.value(),
                                        page: 1,
                                        sort: sort.clone(),
                                        direction,
                                    });
                            },
                            option { value: "", {t!("admin-auctions-world-all")} }
                            option { value: "main", {t!("admin-auctions-world-main")} }
                            option { value: "isekai", {t!("admin-auctions-world-isekai")} }
                        }
                    }
                }
                AdminAuctionCreateForm { notice }
            }
            if let Some(notice) = notice() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {auction_notice_label(notice)}
                }
            }
            {list_content}
        }
    }
}

/// 对当前页拍卖行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
fn sort_auctions(auctions: &mut [AdminAuction], column: &str, direction: SortDirection) {
    if column.is_empty() {
        return;
    }
    auctions.sort_by(|left, right| {
        directed(
            match column {
                "sequence" => left.sequence.cmp(&right.sequence),
                "status" => left.status.cmp(&right.status),
                "open_at" => left.scheduled_open_at.cmp(&right.scheduled_open_at),
                "end_at" => left.scheduled_ends_at.cmp(&right.scheduled_ends_at),
                _ => Ordering::Equal,
            },
            direction,
        )
    });
}
