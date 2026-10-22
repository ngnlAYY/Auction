//! Public 补偿池页面装配。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{
    CompensationMovementPage, CompensationPoolPage, CompensationRound,
    fetch_compensation_movements, fetch_compensation_pool, lot_code_for,
};

use super::{
    AsyncState, AsyncStateKind, PublicPager, QueryWriteMode, Route, Time, format_amount,
    format_auction_time, normalize_public_page_size, public_page_matches,
    update_public_query_state, use_public_query_state, world_label,
};

mod movements;
mod table;

use movements::CompensationMovementDetails;
use table::CompensationPoolTable;

/// 批次与展开流水使用各自的页码/数量；世界、批次与流水身份均通过 URL 恢复。
pub(super) fn pool_page() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let query_state = use_public_query_state();
    let query_state_signal = query_state.state;
    let page_number = use_memo(move || query_state_signal.read().page.max(1));
    let page_size = use_memo(move || query_state_signal.read().page_size);
    let selected_auction_id =
        use_memo(move || query_state_signal.read().auction_id.clone().unwrap_or_default());
    let movement_page = use_memo(move || query_state_signal.read().movement_page);
    let movement_page_size = use_memo(move || query_state_signal.read().movement_page_size);
    let pool = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            let requested = (page_number(), page_size());
            async move {
                let result = fetch_compensation_pool(requested.0, requested.1).await;
                (requested, result)
            }
        }
    });
    let movements = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            let auction_id = selected_auction_id();
            let page = movement_page();
            let page_size = movement_page_size();
            async move {
                let result = if auction_id.is_empty() {
                    Ok(None)
                } else {
                    fetch_compensation_movements(auction_id.clone(), page, page_size)
                        .await
                        .and_then(|page| {
                            if page.round.auction_id == auction_id {
                                Ok(Some(page))
                            } else {
                                Err("invalid_response".to_owned())
                            }
                        })
                };
                (auction_id, page, page_size, result)
            }
        }
    });

    let loaded = pool.read_unchecked();
    let current = loaded
        .as_ref()
        .filter(|(requested, _)| *requested == (page_number(), page_size()))
        .map(|(_, result)| result);
    match current {
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
        Some(Ok(page))
            if !public_page_matches(page.page, page.page_size, page_number(), page_size()) =>
        {
            rsx! {
                p { role: "status", {t!("data-loading")} }
            }
        }
        Some(Ok(page)) => {
            // 只保留同一轮次及同一分页请求的快照，避免旧数据触发新页码纠偏。
            let movement_state =
                movements.read_unchecked().as_ref().and_then(|(auction_id, page, size, result)| {
                    (auction_id == &selected_auction_id()
                        && *page == movement_page()
                        && *size == movement_page_size())
                    .then(|| result.clone())
                });
            rsx! {
                CompensationPoolTable {
                    page: page.clone(),
                    selected_auction_id: selected_auction_id(),
                    movements: movement_state,
                    on_page_change: move |next_page: i64| {
                        update_public_query_state(
                            query_state_signal,
                            |query| {
                                query.page = next_page.max(1);
                                query.auction_id = None;
                                query.movement_page = 1;
                            },
                            QueryWriteMode::Push,
                        );
                    },
                    on_page_clamp: move |next_page: i64| {
                        update_public_query_state(
                            query_state_signal,
                            |query| {
                                query.page = next_page.max(1);
                                query.auction_id = None;
                                query.movement_page = 1;
                            },
                            QueryWriteMode::Replace,
                        );
                    },
                    on_page_size_change: move |size| {
                        update_public_query_state(
                            query_state_signal,
                            |query| {
                                query.page_size = normalize_public_page_size(size);
                                query.page = 1;
                                query.auction_id = None;
                                query.movement_page = 1;
                            },
                            QueryWriteMode::Push,
                        );
                    },
                    on_select_round: move |auction_id: String| {
                        update_public_query_state(
                            query_state_signal,
                            |query| {
                                query.auction_id = (query.auction_id.as_deref() != Some(&auction_id))
                                    .then_some(auction_id);
                                query.movement_page = 1;
                            },
                            QueryWriteMode::Push,
                        );
                    },
                    on_movement_page_change: move |next_page: i64| {
                        update_public_query_state(
                            query_state_signal,
                            |query| query.movement_page = next_page.max(1),
                            QueryWriteMode::Push,
                        );
                    },
                    on_movement_page_clamp: move |next_page: i64| {
                        update_public_query_state(
                            query_state_signal,
                            |query| query.movement_page = next_page.max(1),
                            QueryWriteMode::Replace,
                        );
                    },
                    on_movement_page_size_change: move |size| {
                        update_public_query_state(
                            query_state_signal,
                            |query| {
                                query.movement_page_size = normalize_public_page_size(size);
                                query.movement_page = 1;
                            },
                            QueryWriteMode::Push,
                        );
                    },
                    on_retry_movements: move |_| {
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    },
                }
            }
        }
    }
}
