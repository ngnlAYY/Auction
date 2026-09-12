//! Public 补偿池页面装配。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{
    CompensationMovementPage, CompensationPoolPage, CompensationRound,
    fetch_compensation_movements, fetch_compensation_pool, lot_code_for,
};

use super::{
    AsyncState, AsyncStateKind, Pager, QueryWriteMode, Route, Time, format_amount,
    format_auction_time, update_public_query_state, use_public_query_state, world_label,
};

mod movements;
mod table;

use movements::CompensationMovementDetails;
use table::CompensationPoolTable;

/// 读取公开补偿池批次，并按旧版公开页面展示流水详情。
pub(super) fn pool_page() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    // 池页码与地址栏双向同步：初值取自 URL，浏览器前进/后退由 hook 恢复；流水详情选中态
    // 与流水页码仍是页面本地状态（旧版不入 URL）。
    let query_state = use_public_query_state();
    let query_state_signal = query_state.state;
    let page_number = use_memo(move || query_state_signal().page.max(1));
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
                let result = if auction_id.is_empty() {
                    Ok(None)
                } else {
                    fetch_compensation_movements(auction_id.clone(), page).await.and_then(|page| {
                        if page.round.auction_id == auction_id {
                            Ok(Some(page))
                        } else {
                            Err("invalid_response".to_owned())
                        }
                    })
                };
                (auction_id, result)
            }
        }
    });

    match &*pool.read_unchecked() {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("page-compensation-pool-title"),
                detail: t!("data-loading"),
            }
        },
        Some(Err(_)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("page-compensation-pool-title"),
                detail: t!("data-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                },
            }
        },
        Some(Ok(page)) => {
            // 同轮次刷新保留快照；切换轮次不能把旧流水与新轮次余额、世界拼在一起。
            let movement_state =
                movements.read_unchecked().as_ref().and_then(|(auction_id, result)| {
                    (auction_id == &selected_auction_id()).then(|| result.clone())
                });
            rsx! {
                CompensationPoolTable {
                    page: page.clone(),
                    selected_auction_id: selected_auction_id(),
                    movements: movement_state,
                    on_page_change: move |next_page: i64| {
                        let next_page = next_page.max(1);
                        selected_auction_id.set(String::new());
                        movement_page.set(1);
                        update_public_query_state(
                            query_state_signal,
                            |query| query.page = next_page,
                            QueryWriteMode::Push,
                        );
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
