//! Admin 拍卖列表表格与筛选器。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;

use crate::api::{AdminAuctionLifecycleAction, AdminAuctionPage};

use super::super::intake::AdminIntakeForm;
use super::super::pagination::AdminPager;
use super::actions::{AdminAuctionRow, lifecycle_action_success_label};
use super::create::AdminAuctionCreateForm;
use super::helpers::current_time_millis;

#[component]
pub(super) fn AdminAuctionTable(
    state: Option<Result<AdminAuctionPage, String>>,
    mut world_filter: Signal<String>,
    mut page_number: Signal<i64>,
    reload_generation: Signal<u64>,
) -> Element {
    let clock_millis = use_signal(current_time_millis);
    let notice = use_signal(|| None::<AdminAuctionLifecycleAction>);
    #[cfg(target_arch = "wasm32")]
    let _clock_task = use_future(move || {
        let mut clock_millis = clock_millis;
        async move {
            loop {
                TimeoutFuture::new(1_000).await;
                clock_millis.set(current_time_millis());
            }
        }
    });
    let list_content = match state {
        None => rsx! {
            p { class: "admin-auction-status", role: "status", {t!("admin-auctions-loading")} }
        },
        Some(Err(detail)) => rsx! {
            p { class: "admin-auction-error", role: "alert", "{detail}" }
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
                        thead {
                            tr {
                                th { class: "admin-auction-col-number", {t!("admin-auctions-sequence")} }
                                th { class: "admin-auction-col-world", {t!("admin-auctions-world")} }
                                th { class: "admin-auction-col-status", {t!("admin-auctions-status")} }
                                th { class: "admin-auction-col-opens", {t!("admin-auctions-open-at")} }
                                th { class: "admin-auction-col-ends", {t!("admin-auctions-end-at")} }
                                th { class: "admin-auction-col-ready", {t!("admin-auctions-readiness")} }
                                th { {t!("admin-auctions-actions")} }
                            }
                        }
                        tbody {
                            for auction in page.auctions {
                                AdminAuctionRow {
                                    auction,
                                    capabilities: capabilities.clone(),
                                    clock_millis,
                                    notice,
                                    reload_generation,
                                }
                            }
                        }
                    }
                }
                if is_empty {
                    p { class: "admin-auction-empty admin-auction-status", {t!("admin-auctions-empty")} }
                }
                AdminPager {
                    page: pagination.page,
                    page_size: pagination.page_size,
                    total: pagination.total,
                    label: t!("admin-auctions-pagination"),
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
                AdminAuctionCreateForm { reload_generation }
            }
            if let Some(action) = notice() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {lifecycle_action_success_label(action)}
                }
            }
            {list_content}
        }
    }
}
