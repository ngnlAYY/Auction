//! Admin 拍卖列表表格与筛选器。

use dioxus::prelude::*;
use dioxus_i18n::t;

#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;

use crate::api::{AdminAuctionCapabilities, AdminAuctionLifecycleAction, AdminAuctionPage};

use super::super::intake::AdminIntakeForm;
use super::super::pagination::AdminPager;
use super::actions::{AdminAuctionRow, lifecycle_action_success_label};
use super::create::AdminAuctionCreateForm;
use super::helpers::{admin_auction_status_label, current_time_millis};

#[component]
pub(super) fn AdminAuctionTable(
    page: AdminAuctionPage,
    capabilities: AdminAuctionCapabilities,
    mut world_filter: Signal<String>,
    mut status_filter: Signal<String>,
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
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-auctions-title",
            h1 { id: "admin-auctions-title", {t!("page-auctions-title")} }
            div { class: "admin-panel__actions admin-auctions__filters",
                label {
                    class: "admin-auctions__world-filter",
                    {t!("admin-auctions-world")}
                    select {
                        value: world_filter(),
                        onchange: move |event| {
                            world_filter.set(event.value());
                            page_number.set(1);
                        },
                        option { value: "all", {t!("admin-auctions-world-all")} }
                        option { value: "main", {t!("admin-auctions-world-main")} }
                        option { value: "isekai", {t!("admin-auctions-world-isekai")} }
                    }
                }
                label {
                    class: "admin-auctions__status-filter",
                    {t!("admin-auctions-status")}
                    select {
                        value: status_filter(),
                        onchange: move |event| {
                            status_filter.set(event.value());
                            page_number.set(1);
                        },
                        option { value: "all", {t!("admin-auctions-status-all")} }
                        for status in ["draft", "preview", "live", "final", "settling", "settled", "cancelled"] {
                            option { value: status, {admin_auction_status_label(status)} }
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
            if capabilities.manual_intake && matches!(world_filter().as_str(), "main" | "isekai") {
                AdminIntakeForm {
                    world_id: world_filter(),
                    reload_generation,
                }
            }
            if page.auctions.is_empty() {
                p { class: "admin-panel__note", role: "status", {t!("admin-auctions-empty")} }
            } else {
                div { class: "public-auction-catalog admin-auctions__table-wrap",
                    table {
                        thead {
                            tr {
                                th { {t!("admin-auctions-sequence")} }
                                th { {t!("admin-auctions-world")} }
                                th { {t!("admin-auctions-status")} }
                                th { {t!("admin-auctions-open-at")} }
                                th { {t!("admin-auctions-end-at")} }
                                th { {t!("admin-auctions-readiness")} }
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
            }
            AdminPager {
                page: page.pagination.page,
                page_size: page.pagination.page_size,
                total: page.pagination.total,
                label: t!("page-auctions-title"),
                on_page_change: move |next_page| page_number.set(next_page),
            }
        }
    }
}
