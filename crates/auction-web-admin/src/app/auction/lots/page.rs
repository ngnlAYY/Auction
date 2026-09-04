//! Admin lot 页面加载和错误状态。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind};

use crate::api::fetch_admin_lots;

use super::super::{Route, admin_api_error_message};

/// 加载并展示一个拍卖的 Admin lot 安全投影。
pub fn page(auction_id: String) -> Element {
    let page_number = use_signal(|| 1_i64);
    let reload_generation = use_signal(|| 0_u64);
    let lots = use_resource({
        let auction_id = auction_id.clone();
        move || {
            let _ = reload_generation();
            fetch_admin_lots(auction_id.clone(), page_number())
        }
    });

    match &*lots.read_unchecked() {
        None => rsx! {
            AdminLotLoading {}
        },
        Some(Err(error)) => rsx! {
            AdminLotLoadError {
                detail: admin_api_error_message(error),
                reload_generation,
            }
        },
        Some(Ok(page)) => rsx! {
            super::table::AdminLotTable { page: page.clone(), page_number, reload_generation }
        },
    }
}

#[component]
fn AdminLotLoading() -> Element {
    rsx! {
        section { class: "admin-auction-page", aria_labelledby: "auction-title",
            h1 { id: "auction-title", {t!("page-auction-lots-title", auction: "…")} }
            p { class: "admin-auction-intro",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            section { class: "admin-auction-lot-section", aria_labelledby: "lot-title",
                h2 { id: "lot-title", class: "admin-visually-hidden",
                    {t!("page-auction-lots-title", auction: "…")}
                }
                AsyncState {
                    kind: AsyncStateKind::Loading,
                    title: t!("admin-lots-loading"),
                    detail: "",
                }
            }
        }
    }
}

#[component]
fn AdminLotLoadError(detail: String, mut reload_generation: Signal<u64>) -> Element {
    rsx! {
        section { class: "admin-auction-page", aria_labelledby: "auction-title",
            h1 { id: "auction-title", {t!("page-auction-lots-title", auction: "…")} }
            p { class: "admin-auction-intro",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            section { class: "admin-auction-lot-section", aria_labelledby: "lot-title",
                h2 { id: "lot-title", class: "admin-visually-hidden",
                    {t!("page-auction-lots-title", auction: "…")}
                }
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: detail,
                    detail: "",
                    retry_label: t!("admin-lots-retry"),
                    on_retry: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                }
            }
        }
    }
}
