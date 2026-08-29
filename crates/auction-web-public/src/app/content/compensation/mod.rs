//! Public 补偿池页面装配。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{
    CompensationMovementPage, CompensationPoolPage, CompensationRound,
    fetch_compensation_movements, fetch_compensation_pool, lot_code_for,
};

use super::pagination::PublicPager;
use super::{
    Placeholder, RetryPlaceholder, Route, format_amount, format_auction_time, read_public_query,
    update_public_query, world_label,
};

mod movements;
mod table;

use movements::CompensationMovementDetails;
use table::CompensationPoolTable;

/// 读取公开补偿池批次，并按旧版公开页面展示流水详情。
pub(super) fn pool_page() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let mut page_number = use_signal(|| read_public_query().page.max(1));
    let mut selected_auction_id = use_signal(String::new);
    let mut movement_page = use_signal(|| 1_i64);
    let pool = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            fetch_compensation_pool(page_number())
        }
    });
    let movements = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            let auction_id = selected_auction_id();
            let page = movement_page();
            async move {
                if auction_id.is_empty() {
                    Ok(None)
                } else {
                    fetch_compensation_movements(auction_id, page).await.map(Some)
                }
            }
        }
    });

    match &*pool.read_unchecked() {
        None => rsx! {
            Placeholder {
                title: t!("page-compensation-pool-title"),
                detail: t!("data-loading"),
            }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("page-compensation-pool-title"),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(page)) => {
            let movement_state = movements.read_unchecked().clone();
            rsx! {
                CompensationPoolTable {
                    page: page.clone(),
                    selected_auction_id: selected_auction_id(),
                    movements: movement_state,
                    on_page_change: move |next_page: i64| {
                        let next_page = next_page.max(1);
                        page_number.set(next_page);
                        selected_auction_id.set(String::new());
                        movement_page.set(1);
                        update_public_query(|query| query.page = next_page, false);
                    },
                    on_select_round: move |auction_id: String| {
                        if selected_auction_id() == auction_id {
                            selected_auction_id.set(String::new());
                        } else {
                            selected_auction_id.set(auction_id);
                        }
                        movement_page.set(1);
                    },
                    on_movement_page_change: move |next_page: i64| {
                        movement_page.set(next_page.max(1));
                    },
                    on_retry_movements: move |_| {
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    },
                }
            }
        }
    }
}
