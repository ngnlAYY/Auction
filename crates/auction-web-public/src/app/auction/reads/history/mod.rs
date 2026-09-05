//! Public 价格历史页面及其筛选、值规范化和结果表格。

use super::*;

mod filters;
mod table;
mod values;

pub(crate) use filters::*;
pub(crate) use table::*;
pub(crate) use values::*;
pub(crate) fn history() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    // 筛选与页码与地址栏双向同步：初值取自 URL，浏览器前进/后退由 hook 恢复。
    let query_state = use_public_query_state();
    let query_state_signal = query_state.state;
    // 已应用查询沿用旧版内存 normalize（非法日期/价格/区间被丢弃，但不改写地址栏）。
    let applied_query = use_memo(move || {
        let query = normalize_history_query(query_state_signal());
        PriceHistoryQuery {
            world: query.world.clone(),
            name: query.history_name.clone(),
            from: query.history_from.clone(),
            to: query.history_to.clone(),
            min_price: query.history_min_price.clone(),
            max_price: query.history_max_price.clone(),
            buyer: query.history_buyer.clone(),
            seller: query.history_seller.clone(),
            page: query.page.max(1),
        }
    });
    // 草稿筛选输入以已应用查询为初值；地址栏变化（含前进/后退恢复）时经 effect 重新同步。
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
    let mut settings_open = use_signal(|| false);
    let mut filter_error = use_signal(|| None::<HistoryFilterError>);
    use_effect(move || {
        let applied = applied_query();
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
    });
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => super::supported_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };

    let history = use_resource(move || {
        let _ = refresh_generation();
        fetch_price_history(applied_query())
    });
    let history_unsupported =
        matches!(&*history.read_unchecked(), Some(Err(PriceHistoryFetchError::Unsupported)));
    let body = match &*history.read_unchecked() {
        None => rsx! {
            p { class: "public-auction-page__status", role: "status", {t!("data-loading")} }
        },
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
        Some(Ok(page)) => rsx! {
            PriceHistoryTable {
                page: page.clone(),
                current_page: applied_query().page,
                on_page_change: move |next_page: i64| {
                    let next_page = next_page.max(1);
                    update_public_query_state(
                        query_state_signal,
                        |query| query.page = next_page,
                        QueryWriteMode::Push,
                    );
                },
            }
        },
    };

    let apply_filters = move |_| {
        let mut validation_error = None;
        let from = match history_date_value(&from_month_filter(), &from_year_filter(), false) {
            Ok(value) => value,
            Err(error) => {
                validation_error = Some(match error {
                    HistoryDateError::Incomplete => HistoryFilterError::FromDateIncomplete,
                    HistoryDateError::Invalid => HistoryFilterError::FromDateInvalid,
                });
                None
            }
        };
        let to = match history_date_value(&to_month_filter(), &to_year_filter(), true) {
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
                            HistoryPriceUnit::K => HistoryFilterError::MaximumPriceK,
                            HistoryPriceUnit::M => HistoryFilterError::MaximumPriceM,
                        });
                    }
                    None
                }
            };
        if validation_error.is_none()
            && minimum_price
                .as_deref()
                .zip(maximum_price.as_deref())
                .is_some_and(|(minimum, maximum)| minimum > maximum)
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
            name: optional_filter_value(&name_filter()),
            from,
            to,
            min_price: minimum_price,
            max_price: maximum_price,
            buyer: optional_filter_value(&buyer_filter()),
            seller: optional_filter_value(&seller_filter()),
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
                query.page = 1;
            },
            QueryWriteMode::Push,
        );
    };
    let active_filter_count = history_active_filter_count(&applied_query());

    rsx! {
        section { class: "public-history-page", aria_labelledby: "price-history-title",
            h1 { id: "price-history-title", {t!("page-history-title")} }
            p { class: "public-history-intro", {t!("page-history-note")} }
            if !history_unsupported {
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
            {body}
        }
    }
}
