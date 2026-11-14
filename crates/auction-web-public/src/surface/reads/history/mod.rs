//! Public 价格历史页面及其筛选、值规范化和结果表格。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::{PriceHistoryFetchError, PriceHistoryQuery, fetch_price_history, fetch_worlds};
use crate::surface::{
    AsyncState, AsyncStateKind, QueryWriteMode, public_page_matches, update_public_page_size,
    update_public_query_state, use_content_loading, use_public_query_state,
};
use crate::surface::{LotDetailLookupHost, use_lot_detail_lookup};

mod filters;
mod presentation;
mod table;
mod values;

use self::presentation::HistoryResultControls;
pub(crate) use filters::*;
pub(crate) use table::*;
pub(crate) use values::*;
pub(crate) fn history() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    // 成交行的查看按钮在页内打开只读的拍品详情抽屉；读取任务归页面宿主，不随翻页卸载。
    let lookup = use_lot_detail_lookup();
    // 筛选与页码与地址栏双向同步。
    let query_state = use_public_query_state();
    let query_state_signal = query_state.state;
    let page_size = use_memo(move || query_state_signal.read().page_size);
    // 日期沿用既有恢复规则；非法名称/金额保留为字段错误并阻止查询，不改写地址栏。
    let applied_query = use_memo(move || {
        let query = normalize_history_query(query_state_signal());
        PriceHistoryQuery {
            world: query.world,
            name: query.history_name,
            from: query.history_from,
            to: query.history_to,
            min_price: query.history_min_price,
            max_price: query.history_max_price,
            buyer: query.history_buyer,
            seller: query.history_seller,
            sort: query.history_sort,
            page: query.page.max(1),
        }
    });
    // 草稿只随实际筛选条件恢复；翻页与排序不应覆盖尚未提交的输入或字段错误。
    let applied_filters = use_memo(move || {
        let mut filters = applied_query();
        filters.sort.clear();
        filters.page = 1;
        filters
    });
    // 筛选草稿以已应用查询为初值，筛选条件变化（含前进/后退恢复）时经 effect 同步。
    let initial = applied_query.peek().clone();
    let mut world_filter = use_signal(|| initial.world.clone().unwrap_or_else(|| "all".to_owned()));
    let mut name_filter = use_signal(|| initial.name.clone().unwrap_or_default());
    let (from_month, from_year) = history_date_parts(initial.from.as_deref());
    let (to_month, to_year) = history_date_parts(initial.to.as_deref());
    let (minimum_price, minimum_price_unit) = history_price_parts(initial.min_price.as_deref());
    let (maximum_price, maximum_price_unit) = history_price_parts(initial.max_price.as_deref());
    let mut from_month_filter = use_signal(|| from_month);
    let mut from_year_filter = use_signal(|| from_year);
    let mut to_month_filter = use_signal(|| to_month);
    let mut to_year_filter = use_signal(|| to_year);
    let mut minimum_price_filter = use_signal(|| minimum_price);
    let mut minimum_price_unit_filter = use_signal(|| minimum_price_unit);
    let mut maximum_price_filter = use_signal(|| maximum_price);
    let mut maximum_price_unit_filter = use_signal(|| maximum_price_unit);
    let mut buyer_filter = use_signal(|| initial.buyer.clone().unwrap_or_default());
    let mut seller_filter = use_signal(|| initial.seller.clone().unwrap_or_default());
    let initial_error = history_query_error(&initial);
    let mut settings_open = use_signal(|| {
        matches!(
            initial_error,
            Some(
                HistoryFilterError::MinimumPriceC
                    | HistoryFilterError::MaximumPriceC
                    | HistoryFilterError::PriceRange
            )
        )
    });
    let mut filter_error = use_signal(|| initial_error);
    use_effect(move || {
        let applied = applied_filters();
        world_filter.set(applied.world.clone().unwrap_or_else(|| "all".to_owned()));
        name_filter.set(applied.name.clone().unwrap_or_default());
        let (from_month, from_year) = history_date_parts(applied.from.as_deref());
        from_month_filter.set(from_month);
        from_year_filter.set(from_year);
        let (to_month, to_year) = history_date_parts(applied.to.as_deref());
        to_month_filter.set(to_month);
        to_year_filter.set(to_year);
        let (minimum_price, minimum_price_unit) = history_price_parts(applied.min_price.as_deref());
        minimum_price_filter.set(minimum_price);
        minimum_price_unit_filter.set(minimum_price_unit);
        let (maximum_price, maximum_price_unit) = history_price_parts(applied.max_price.as_deref());
        maximum_price_filter.set(maximum_price);
        maximum_price_unit_filter.set(maximum_price_unit);
        buyer_filter.set(applied.buyer.clone().unwrap_or_default());
        seller_filter.set(applied.seller.clone().unwrap_or_default());
        let error = history_query_error(&applied);
        if matches!(
            error,
            Some(
                HistoryFilterError::MinimumPriceC
                    | HistoryFilterError::MaximumPriceC
                    | HistoryFilterError::PriceRange
            )
        ) {
            settings_open.set(true);
        }
        filter_error.set(error);
    });
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => super::supported_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };

    let active_filter_count = history_active_filter_count(&applied_query());
    let has_search = active_filter_count > 0 && history_query_error(&applied_query()).is_none();
    let history = use_resource(move || {
        let _ = refresh_generation();
        let query = applied_query();
        let requested_page_size = page_size();
        async move {
            // 空查询只展示搜索入口；输入草稿在提交并写回 URL 后才触发权威历史读取。
            if history_active_filter_count(&query) == 0 || history_query_error(&query).is_some() {
                return None;
            }
            let result = fetch_price_history(query.clone(), requested_page_size).await;
            Some((query, requested_page_size, result))
        }
    });
    // Resource 会保留旧结果；重置或切换查询后不能把前一次装备列表装配到当前搜索条件。
    let history_snapshot = history.read_unchecked();
    let history_result = history_snapshot
        .as_ref()
        .and_then(Option::as_ref)
        .filter(|(query, size, _)| *query == applied_query() && *size == page_size())
        .map(|(_, _, result)| result);
    let history_unsupported =
        matches!(history_result, Some(Err(PriceHistoryFetchError::Unsupported)));
    let table_scope_key = format!("{:?}:{}", applied_query(), page_size());
    // 已提交的搜索还没有结果（含等待页码纠偏后的重读）时由顶栏提示加载，正文留空。
    use_content_loading(
        has_search
            && match history_result {
                None => true,
                Some(Ok(page)) => !public_page_matches(
                    page.page,
                    page.page_size,
                    applied_query().page,
                    page_size(),
                ),
                Some(Err(_)) => false,
            },
    );
    let body = if !has_search {
        rsx! {}
    } else {
        match history_result {
            None => rsx! {},
            Some(Err(PriceHistoryFetchError::Unsupported)) => rsx! {
                AsyncState {
                    kind: AsyncStateKind::Empty,
                    title: t!("page-history-title"),
                    detail: t!("page-history-unsupported"),
                }
            },
            Some(Err(PriceHistoryFetchError::Unavailable)) => rsx! {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: t!("page-history-title"),
                    detail: t!("data-error"),
                    retry_label: t!("data-retry"),
                    on_retry: move |_| {
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    },
                }
            },
            Some(Ok(page))
                if !public_page_matches(
                    page.page,
                    page.page_size,
                    applied_query().page,
                    page_size(),
                ) =>
            {
                rsx! {}
            }
            Some(Ok(page)) => rsx! {
                // 查询身份同时隔离子分组的折叠状态；同查询后台重读保留当前展开选择。
                for (table_scope_key, page) in [(table_scope_key, page.clone())] {
                    PriceHistoryTable {
                        key: "{table_scope_key}",
                        page,
                        grouping: query_state_signal().history_group,
                        current_page: applied_query().page,
                        on_buyer_filter: move |name| {
                            update_public_query_state(query_state_signal, |query| {
                                query.history_buyer = Some(name);
                                query.page = 1;
                            }, QueryWriteMode::Push);
                        },
                        on_seller_filter: move |name| {
                            update_public_query_state(query_state_signal, |query| {
                                query.history_seller = Some(name);
                                query.page = 1;
                            }, QueryWriteMode::Push);
                        },
                        on_page_size_change: move |size| update_public_page_size(query_state_signal, size),
                        on_page_change: move |next_page: i64| {
                            let next_page = next_page.max(1);
                            update_public_query_state(
                                query_state_signal,
                                |query| query.page = next_page,
                                QueryWriteMode::Push,
                            );
                        },
                        on_page_clamp: move |next_page: i64| {
                            update_public_query_state(
                                query_state_signal,
                                |query| query.page = next_page.max(1),
                                QueryWriteMode::Replace,
                            );
                        },
                    }
                }
            },
        }
    };

    let apply_filters = move |_| {
        let name = match history_name_value(&name_filter()) {
            Ok(value) => value,
            Err(error) => {
                filter_error.set(Some(error));
                return;
            }
        };
        // 每次提交都以当前已应用查询为锚点，路由恢复和前一次提交不会留下旧日期边界。
        let applied = applied_query();
        let mut validation_error = None;
        let from = match history_date_value(
            &from_month_filter(),
            &from_year_filter(),
            false,
            applied.from.as_deref(),
        ) {
            Ok(value) => value,
            Err(error) => {
                validation_error = Some(match error {
                    HistoryDateError::Incomplete => HistoryFilterError::FromDateIncomplete,
                    HistoryDateError::Invalid => HistoryFilterError::FromDateInvalid,
                });
                None
            }
        };
        let to = match history_date_value(
            &to_month_filter(),
            &to_year_filter(),
            true,
            applied.to.as_deref(),
        ) {
            Ok(value) => value,
            Err(error) => {
                if validation_error.is_none() {
                    validation_error = Some(match error {
                        HistoryDateError::Incomplete => HistoryFilterError::ToDateIncomplete,
                        HistoryDateError::Invalid => HistoryFilterError::ToDateInvalid,
                    });
                }
                None
            }
        };
        if validation_error.is_none()
            && from.as_deref().zip(to.as_deref()).is_some_and(|(from, to)| from > to)
        {
            validation_error = Some(HistoryFilterError::DateRange);
        }
        let minimum_price =
            match history_price_value(&minimum_price_filter(), minimum_price_unit_filter()) {
                Ok(value) => value,
                Err(HistoryPriceError::Invalid) => {
                    if validation_error.is_none() {
                        validation_error = Some(match minimum_price_unit_filter() {
                            HistoryPriceUnit::C => HistoryFilterError::MinimumPriceC,
                            HistoryPriceUnit::K => HistoryFilterError::MinimumPriceK,
                            HistoryPriceUnit::M => HistoryFilterError::MinimumPriceM,
                        });
                    }
                    None
                }
            };
        let maximum_price =
            match history_price_value(&maximum_price_filter(), maximum_price_unit_filter()) {
                Ok(value) => value,
                Err(HistoryPriceError::Invalid) => {
                    if validation_error.is_none() {
                        validation_error = Some(match maximum_price_unit_filter() {
                            HistoryPriceUnit::C => HistoryFilterError::MaximumPriceC,
                            HistoryPriceUnit::K => HistoryFilterError::MaximumPriceK,
                            HistoryPriceUnit::M => HistoryFilterError::MaximumPriceM,
                        });
                    }
                    None
                }
            };
        if validation_error.is_none()
            && history_price_range_reversed(minimum_price.as_deref(), maximum_price.as_deref())
        {
            validation_error = Some(HistoryFilterError::PriceRange);
        }
        if let Some(error) = validation_error {
            filter_error.set(Some(error));
            settings_open.set(true);
            return;
        }
        filter_error.set(None);
        let next_query = PriceHistoryQuery {
            world: (world_filter() != "all").then_some(world_filter()),
            name,
            from,
            to,
            min_price: minimum_price,
            max_price: maximum_price,
            buyer: optional_filter_value(&buyer_filter()),
            seller: optional_filter_value(&seller_filter()),
            sort: applied.sort,
            page: 1,
        };
        update_public_query_state(
            query_state_signal,
            |query| {
                query.world = next_query.world.clone();
                query.history_name = next_query.name.clone();
                query.history_from = next_query.from.clone();
                query.history_to = next_query.to.clone();
                query.history_min_price = next_query.min_price.clone();
                query.history_max_price = next_query.max_price.clone();
                query.history_buyer = next_query.buyer.clone();
                query.history_seller = next_query.seller.clone();
                query.page = 1;
            },
            QueryWriteMode::Push,
        );
    };
    let reset_filters = move |_| {
        world_filter.set("all".to_owned());
        name_filter.set(String::new());
        from_month_filter.set(String::new());
        from_year_filter.set(String::new());
        to_month_filter.set(String::new());
        to_year_filter.set(String::new());
        minimum_price_filter.set(String::new());
        minimum_price_unit_filter.set(HistoryPriceUnit::K);
        maximum_price_filter.set(String::new());
        maximum_price_unit_filter.set(HistoryPriceUnit::K);
        buyer_filter.set(String::new());
        seller_filter.set(String::new());
        settings_open.set(false);
        filter_error.set(None);
        update_public_query_state(
            query_state_signal,
            |query| {
                query.world = None;
                query.history_name = None;
                query.history_from = None;
                query.history_to = None;
                query.history_min_price = None;
                query.history_max_price = None;
                query.history_buyer = None;
                query.history_seller = None;
                query.history_sort = "closed_desc".to_owned();
                query.history_group = "equipment".to_owned();
                query.page = 1;
            },
            QueryWriteMode::Push,
        );
    };
    rsx! {
        section { class: "public-history-page", aria_label: t!("page-history-title"),
            if !history_unsupported {
                div { class: "public-history-search-region",
                    PriceHistoryFilters {
                        world: world_filter,
                        name: name_filter,
                        from_month: from_month_filter,
                        from_year: from_year_filter,
                        to_month: to_month_filter,
                        to_year: to_year_filter,
                        minimum_price: minimum_price_filter,
                        minimum_price_unit: minimum_price_unit_filter,
                        maximum_price: maximum_price_filter,
                        maximum_price_unit: maximum_price_unit_filter,
                        buyer: buyer_filter,
                        seller: seller_filter,
                        world_options,
                        active_filter_count,
                        settings_open,
                        filter_error,
                        on_apply: apply_filters,
                        on_reset: reset_filters,
                    }
                }
            }
            if has_search {
                div { class: "public-history-output",
                    HistoryResultControls {
                        sort: applied_query().sort,
                        grouping: query_state_signal().history_group,
                        on_sort_change: move |sort| {
                            update_public_query_state(query_state_signal, |query| {
                                query.history_sort = sort;
                                query.page = 1;
                            }, QueryWriteMode::Push);
                        },
                        on_group_change: move |group| {
                            // 分组只重排已取回的本页，不能改变 API 查询或重新读取相同成交集合。
                            update_public_query_state(query_state_signal, |query| {
                                query.history_group = group;
                            }, QueryWriteMode::Push);
                        },
                    }
                    {body}
                }
            }
            if lookup.failed() {
                p { class: "action-feedback action-feedback--error", role: "alert", {t!("data-error")} }
            }
            // 抽屉宿主在结果读取分支之外：翻页、筛选或后台重读替换表格时，已打开的详情不被卸载。
            LotDetailLookupHost { lookup }
        }
    }
}
