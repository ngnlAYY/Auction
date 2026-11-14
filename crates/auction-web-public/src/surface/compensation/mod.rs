//! Public 补偿池页面装配：余额、场次批次与逐件结算明细。

use auction_web_shared::{format_credit_amount_full, t};
use dioxus::prelude::*;

use crate::api::{
    CompensationLotPage, CompensationPoolPage, CompensationRound, fetch_compensation_lots,
    fetch_compensation_pool, lot_code_for,
};

use super::{
    AsyncState, AsyncStateKind, LotDetailLookupButton, LotDetailLookupHost, PublicPager,
    QueryWriteMode, Time, format_auction_time, normalize_public_page_size, public_page_matches,
    update_public_query_state, use_content_loading, use_lot_detail_lookup, use_public_query_state,
    world_label,
};

mod lots;
mod table;

use lots::CompensationLotDetails;
use table::CompensationPoolTable;

/// 批次与展开明细使用各自的页码/数量；批次与明细分页均通过 URL 恢复。
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
    // 逐件明细的查看按钮在页内打开只读的拍品详情抽屉；读取任务归页面宿主，不随翻页卸载。
    let lookup = use_lot_detail_lookup();
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
    let lots = use_resource({
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
                    fetch_compensation_lots(auction_id.clone(), page, page_size).await.and_then(
                        |page| {
                            if page.round.auction_id == auction_id {
                                Ok(Some(page))
                            } else {
                                Err("invalid_response".to_owned())
                            }
                        },
                    )
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
    // 没有当前页码的结果（含等待页码纠偏后的重读）时由顶栏提示加载，正文留空。
    use_content_loading(match current {
        None => true,
        Some(Ok(page)) => {
            !public_page_matches(page.page, page.page_size, page_number(), page_size())
        }
        Some(Err(_)) => false,
    });
    let body = match current {
        None => rsx! {},
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
            rsx! {}
        }
        Some(Ok(page)) => {
            // 只保留同一轮次及同一分页请求的快照，避免旧数据触发新页码纠偏。
            let lot_state =
                lots.read_unchecked().as_ref().and_then(|(auction_id, page, size, result)| {
                    (auction_id == &selected_auction_id()
                        && *page == movement_page()
                        && *size == movement_page_size())
                    .then(|| result.clone())
                });
            rsx! {
                CompensationPoolTable {
                    page: page.clone(),
                    selected_auction_id: selected_auction_id(),
                    lots: lot_state,
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
                    on_lot_page_change: move |next_page: i64| {
                        update_public_query_state(
                            query_state_signal,
                            |query| query.movement_page = next_page.max(1),
                            QueryWriteMode::Push,
                        );
                    },
                    on_lot_page_clamp: move |next_page: i64| {
                        update_public_query_state(
                            query_state_signal,
                            |query| query.movement_page = next_page.max(1),
                            QueryWriteMode::Replace,
                        );
                    },
                    on_lot_page_size_change: move |size| {
                        update_public_query_state(
                            query_state_signal,
                            |query| {
                                query.movement_page_size = normalize_public_page_size(size);
                                query.movement_page = 1;
                            },
                            QueryWriteMode::Push,
                        );
                    },
                    on_retry_lots: move |_| {
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    },
                }
            }
        }
    };
    rsx! {
        {body}
        if lookup.failed() {
            p { class: "action-feedback action-feedback--error", role: "alert", {t!("data-error")} }
        }
        // 抽屉宿主在批次读取分支之外：翻页或后台重读替换表格时，已打开的详情不被卸载。
        LotDetailLookupHost { lookup }
    }
}

/// 补偿池页面统一展示精确金额，不把整千值缩写为 K/M，避免同一列混用单位影响复算。
fn credits(amount: i64) -> String {
    format!("{}c", format_credit_amount_full(amount))
}

/// 以补偿池视角带符号展示金额：流入为 `+`，流出为 `−`。
fn signed_amount(amount: i64) -> String {
    if amount > 0 {
        format!("+{}", credits(amount))
    } else if amount < 0 {
        format!("−{}", credits(amount.saturating_abs()))
    } else {
        credits(0)
    }
}

/// 实际兑付率：补偿支出占名义折扣需求的比例，精确到 0.01%；没有需求时不适用。
fn coverage_label(compensation_out: i64, discount_demand: i64) -> String {
    if discount_demand <= 0 {
        return "—".to_owned();
    }
    let ppm = i128::from(compensation_out.max(0)) * 1_000_000 / i128::from(discount_demand);
    let ppm = i32::try_from(ppm.min(1_000_000) / 100 * 100).unwrap_or(1_000_000);
    auction_web_shared::format_ppm(ppm, false)
}
