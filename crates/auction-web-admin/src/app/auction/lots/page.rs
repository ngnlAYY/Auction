//! Admin lot 页面加载和错误状态。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::fetch_admin_lots;

use super::super::{AdminPanel, Route};

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
            AdminPanel {
                title: t!("page-auction-lots-title", auction: "…"),
                detail: t!("admin-lots-loading"),
            }
        },
        Some(Err(_)) => rsx! {
            AdminLotLoadError { reload_generation }
        },
        Some(Ok(page)) => rsx! {
            super::table::AdminLotTable { page: page.clone(), page_number, reload_generation }
        },
    }
}

#[component]
fn AdminLotLoadError(mut reload_generation: Signal<u64>) -> Element {
    rsx! {
        section { class: "admin-panel", role: "alert", aria_labelledby: "admin-lots-error-title",
            h1 { id: "admin-lots-error-title", {t!("page-auction-lots-title", auction: "…")} }
            p { class: "admin-lots__back",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            p { class: "admin-panel__note", {t!("admin-lots-error")} }
            button {
                r#type: "button",
                onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                {t!("admin-lots-retry")}
            }
        }
    }
}
