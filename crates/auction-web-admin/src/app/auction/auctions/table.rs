//! Admin 拍卖列表表格与筛选器。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind, Pager, use_second_tick};

use crate::api::AdminAuctionPage;

use super::super::intake::AdminIntakeForm;
use super::actions::{AdminAuctionNotice, AdminAuctionRow, auction_notice_label};
use super::create::AdminAuctionCreateForm;
use super::helpers::current_time_millis;

#[component]
pub(super) fn AdminAuctionTable(
    state: Option<Result<AdminAuctionPage, String>>,
    mut world_filter: Signal<String>,
    mut page_number: Signal<i64>,
    reload_generation: Signal<u64>,
) -> Element {
    // 订阅外壳提供的共享秒级时钟：读取 tick 让本组件每秒重渲染并刷新当前时间，
    // 不再为这张表单独起一个 1 秒定时器。
    let second_tick = use_second_tick();
    let _ = second_tick();
    let current_time_millis = current_time_millis();
    let notice = use_signal(|| None::<AdminAuctionNotice>);
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
            let pagination = page.pagination;
            rsx! {
                if capabilities.manual_intake && matches!(world_filter().as_str(), "main" | "isekai") {
                    AdminIntakeForm {
                        world_id: world_filter(),
                        reload_generation,
                    }
                }
                div { class: "admin-auction-table-wrap",
                    table { class: "admin-auction-table",
                        caption { class: "admin-visually-hidden", {t!("page-auctions-title")} }
                        thead {
                            tr {
                                th { scope: "col", class: "admin-auction-col-number", {t!("admin-auctions-sequence")} }
                                th { scope: "col", class: "admin-auction-col-world", {t!("admin-auctions-world")} }
                                th { scope: "col", class: "admin-auction-col-status", {t!("admin-auctions-status")} }
                                th { scope: "col", class: "admin-auction-col-opens", {t!("admin-auctions-open-at")} }
                                th { scope: "col", class: "admin-auction-col-ends", {t!("admin-auctions-end-at")} }
                                th { scope: "col", class: "admin-auction-col-ready", {t!("admin-auctions-readiness")} }
                                th { scope: "col", {t!("admin-auctions-actions")} }
                            }
                        }
                        tbody {
                            for auction in page.auctions {
                                // 按业务 ID 固定 key，避免刷新/重排后行内本地状态错位绑定到另一行。
                                AdminAuctionRow {
                                    key: "{auction.id}",
                                    auction,
                                    capabilities: capabilities.clone(),
                                    clock_millis: current_time_millis,
                                    notice,
                                    reload_generation,
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
                    page: pagination.page,
                    page_size: pagination.page_size,
                    total: Some(pagination.total),
                    label: t!("admin-auctions-pagination"),
                    previous_label: t!("admin-pagination-previous"),
                    status: move |(page, pages): (i64, i64)| {
                        t!("admin-pagination-status", page: page, pages: pages)
                    },
                    next_label: t!("admin-pagination-next"),
                    jump_label: t!("admin-pagination-input"),
                    jump_button_label: t!("admin-pagination-go"),
                    on_page_change: move |next_page| page_number.set(next_page),
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
                            value: world_filter(),
                            onchange: move |event| {
                                world_filter.set(event.value());
                                page_number.set(1);
                            },
                            option { value: "", {t!("admin-auctions-world-all")} }
                            option { value: "main", {t!("admin-auctions-world-main")} }
                            option { value: "isekai", {t!("admin-auctions-world-isekai")} }
                        }
                    }
                }
                AdminAuctionCreateForm { reload_generation, notice }
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
