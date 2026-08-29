//! Public 价格历史页面及其筛选、值规范化和结果表格。

use super::*;

mod filters;
mod table;
mod values;

pub(crate) use filters::*;
pub(crate) use table::*;
pub(crate) use values::*;
pub(crate) fn history() -> Element {
    let mut selected_world = use_context::<Signal<String>>();
    let refresh_generation = use_context::<Signal<u64>>();
    let query = normalize_history_query(read_public_query());
    let mut world_filter = use_signal(|| query.world.clone().unwrap_or_else(|| "all".to_owned()));
    let mut name_filter = use_signal(|| query.history_name.clone().unwrap_or_default());
    let (from_month, from_year) = history_date_parts(query.history_from.as_deref());
    let (to_month, to_year) = history_date_parts(query.history_to.as_deref());
    let (minimum_price, minimum_price_unit) =
        history_price_parts(query.history_min_price.as_deref());
    let (maximum_price, maximum_price_unit) =
        history_price_parts(query.history_max_price.as_deref());
    let mut from_month_filter = use_signal(|| from_month);
    let mut from_year_filter = use_signal(|| from_year);
    let mut to_month_filter = use_signal(|| to_month);
    let mut to_year_filter = use_signal(|| to_year);
    let mut minimum_price_filter = use_signal(|| minimum_price);
    let mut minimum_price_unit_filter = use_signal(|| minimum_price_unit);
    let mut maximum_price_filter = use_signal(|| maximum_price);
    let mut maximum_price_unit_filter = use_signal(|| maximum_price_unit);
    let mut buyer_filter = use_signal(|| query.history_buyer.clone().unwrap_or_default());
    let mut seller_filter = use_signal(|| query.history_seller.clone().unwrap_or_default());
    let mut settings_open = use_signal(|| false);
    let mut filter_error = use_signal(|| None::<HistoryFilterError>);
    let mut page_number = use_signal(|| query.page.max(1));
    let mut applied_query = use_signal(|| PriceHistoryQuery {
        world: query.world.clone(),
        name: query.history_name.clone(),
        from: query.history_from.clone(),
        to: query.history_to.clone(),
        min_price: query.history_min_price.clone(),
        max_price: query.history_max_price.clone(),
        buyer: query.history_buyer.clone(),
        seller: query.history_seller.clone(),
        page: query.page.max(1),
    });
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => super::enabled_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };

    // 页头世界切换器与旧版筛选共用 world URL 参数；切换页头后同步历史筛选并回到第一页。
    let initial_world = selected_world();
    let mut observed_world = use_signal(|| initial_world);
    use_effect(move || {
        let current_world = selected_world();
        if observed_world() == current_world {
            return;
        }
        observed_world.set(current_world.clone());
        world_filter.set(current_world.clone());
        let mut next_query = applied_query();
        next_query.world = Some(current_world.clone());
        next_query.page = 1;
        applied_query.set(next_query);
        page_number.set(1);
        update_public_query(
            |query| {
                query.world = Some(current_world.clone());
                query.page = 1;
            },
            true,
        );
    });

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
            Placeholder {
                title: t!("page-history-title"),
                detail: t!("page-history-unsupported"),
            }
        },
        Some(Err(PriceHistoryFetchError::Unavailable)) => rsx! {
            RetryPlaceholder {
                title: t!("page-history-title"),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(page)) => rsx! {
            PriceHistoryTable {
                page: page.clone(),
                current_page: page_number(),
                on_page_change: move |next_page: i64| {
                    let next_page = next_page.max(1);
                    page_number.set(next_page);
                    let mut next_query = applied_query();
                    next_query.page = next_page;
                    applied_query.set(next_query);
                    update_public_query(|query| query.page = next_page, false);
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
        if let Some(world) = next_query.world.clone() {
            selected_world.set(world);
        }
        page_number.set(1);
        applied_query.set(next_query.clone());
        update_public_query(
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
            false,
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
        page_number.set(1);
        applied_query.set(PriceHistoryQuery { page: 1, ..PriceHistoryQuery::default() });
        update_public_query(
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
            false,
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
