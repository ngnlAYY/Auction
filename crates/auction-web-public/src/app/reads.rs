//! Public 只读历史页面。
//!
//! 这些页面只消费稳定 HTTP DTO；分页游标和统计语义仍由 Public API 决定，浏览器不重建业务
//! 状态，也不把数据库模型带入组件。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{
    BidLogEntry, BidLogPage, PriceHistoryEntry, PriceHistoryPage, PriceHistoryQuery,
    PriceHistoryStats, fetch_bid_log_auctions, fetch_bid_log_page, fetch_price_history,
    fetch_worlds, lot_code_for,
};

use super::pagination::PublicPager;
use super::{
    CatalogParticipant, Placeholder, RetryPlaceholder, Route, format_auction_time,
    read_public_query, update_public_query,
};

pub(super) fn bid_log() -> Element {
    let selected_world = use_context::<Signal<String>>();
    let mut refresh_generation = use_context::<Signal<u64>>();
    let query = read_public_query();
    let mut selected_auction_id = use_signal(|| query.auction_id.clone().unwrap_or_default());
    let mut page_number = use_signal(|| query.bid_log_page);
    let mut lot_filter = use_signal(|| query.bid_log_lot.clone().unwrap_or_default());
    let auctions = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            fetch_bid_log_auctions(selected_world())
        }
    });

    // 世界切换或分享链接指向另一世界时，自动落到该世界最新一场可公开读取的拍卖。
    use_effect({
        let mut selected_auction_id = selected_auction_id;
        let mut page_number = page_number;
        move || {
            let Some(Ok(auction_list)) = &*auctions.read_unchecked() else {
                return;
            };
            let next_auction_id = auction_list
                .iter()
                .find(|auction| auction.id == selected_auction_id())
                .or_else(|| auction_list.first())
                .map(|auction| auction.id.clone());
            let next_auction_id = next_auction_id.unwrap_or_default();
            if selected_auction_id() != next_auction_id {
                selected_auction_id.set(next_auction_id.clone());
                page_number.set(1);
                update_public_query(
                    |query| {
                        query.auction_id =
                            (!next_auction_id.is_empty()).then_some(next_auction_id.clone());
                        query.bid_log_page = 1;
                    },
                    true,
                );
            }
        }
    });

    let bid_log = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            let auction_id = selected_auction_id();
            let page = page_number();
            let lot_id = lot_filter().trim().to_owned();
            let lot_id = Uuid::parse_str(&lot_id).ok().map(|id| id.to_string());
            async move {
                if auction_id.is_empty() {
                    Ok(None)
                } else {
                    fetch_bid_log_page(auction_id, page, selected_world(), lot_id).await.map(Some)
                }
            }
        }
    });
    let auction_options = match &*auctions.read_unchecked() {
        Some(Ok(auction_list)) => auction_list.clone(),
        _ => Vec::new(),
    };
    let selected_auction =
        auction_options.iter().find(|auction| auction.id == selected_auction_id()).cloned();
    let current_page = page_number();
    let current_lot_filter = lot_filter();
    let body = match &*bid_log.read_unchecked() {
        None => rsx! {
            p { class: "public-auction-bid-log-empty", role: "status", {t!("data-loading")} }
        },
        Some(Err(_)) => rsx! {
            div { class: "public-auction-bid-log-empty", role: "alert",
                p { {t!("data-error")} }
                button {
                    r#type: "button",
                    onclick: move |_| refresh_generation.set(refresh_generation().saturating_add(1)),
                    {t!("data-retry")}
                }
            }
        },
        Some(Ok(None)) => rsx! {
            p { class: "public-auction-bid-log-empty", role: "status", {t!("data-empty")} }
        },
        Some(Ok(Some(page))) if page.bids.is_empty() => rsx! {
            p { class: "public-auction-bid-log-empty", role: "status", {t!("data-empty")} }
        },
        Some(Ok(Some(page))) => rsx! {
            BidLogTable {
                page: page.clone(),
                current_page,
                lot_filter: current_lot_filter.clone(),
                on_lot_filter_change: move |value: String| {
                    lot_filter.set(value.clone());
                    page_number.set(1);
                    update_public_query(
                        |query| query.bid_log_lot = (!value.trim().is_empty()).then_some(value),
                        false,
                    );
                },
                on_page_change: move |next_page: i64| {
                    page_number.set(next_page);
                    update_public_query(|query| query.bid_log_page = next_page, false);
                },
            }
        },
    };

    rsx! {
        section { class: "public-auction-bid-log-page", aria_labelledby: "bid-log-title",
            h1 { id: "bid-log-title", class: "public-auction-category__title",
                {t!("page-bidlog-title")}
            }
            div { class: "public-auction-bid-log-controls",
                label { r#for: "public-auction-bid-log-round",
                    span { {t!("bidlog-round")} }
                    select {
                        id: "public-auction-bid-log-round",
                        value: selected_auction_id(),
                        disabled: auction_options.is_empty(),
                        onchange: move |event| {
                            let auction_id = event.value();
                            selected_auction_id.set(auction_id.clone());
                            page_number.set(1);
                            update_public_query(
                                |query| {
                                    query.auction_id = Some(auction_id.clone());
                                    query.bid_log_page = 1;
                                },
                                false,
                            );
                        },
                        for auction in auction_options.clone() {
                            option {
                                value: auction.id.clone(),
                                {t!("bidlog-round-option", number: auction.sequence,
                                    status: bid_log_status_label(&auction.status))}
                            }
                        }
                    }
                }
            }
            if let Some(auction) = selected_auction {
                div { class: "public-auction-bid-log-round-header",
                    strong { class: "public-auction-bid-log-round-number",
                        {t!("bidlog-round-number", number: auction.sequence)}
                    }
                    span { class: "public-auction-bid-log-round-world",
                        {t!("bidlog-world", world: super::world_label(&auction.world_id))}
                    }
                    span { class: bid_log_status_class(&auction.status),
                        {bid_log_status_label(&auction.status)}
                    }
                }
            }
            {body}
        }
    }
}

#[component]
fn BidLogTable(
    page: BidLogPage,
    current_page: i64,
    lot_filter: String,
    on_lot_filter_change: EventHandler<String>,
    on_page_change: EventHandler<i64>,
) -> Element {
    let has_more = page.has_more.unwrap_or(false);
    let needle = lot_filter.trim().to_ascii_lowercase();
    let server_filtered = Uuid::parse_str(lot_filter.trim()).is_ok();
    let bids = if needle.is_empty() || server_filtered {
        page.bids.clone()
    } else {
        page.bids
            .iter()
            .filter(|bid| {
                lot_code_for(&bid.equipment_category, bid.lot_number)
                    .to_ascii_lowercase()
                    .contains(&needle)
            })
            .cloned()
            .collect()
    };
    rsx! {
        div {
            div { class: "public-auction-bid-log-toolbar",
                label { r#for: "public-auction-bid-log-filter", {t!("bidlog-filter-label")} }
                input {
                    id: "public-auction-bid-log-filter",
                    value: lot_filter.clone(),
                    placeholder: t!("bidlog-filter-placeholder"),
                    oninput: move |event| on_lot_filter_change.call(event.value()),
                }
                if !lot_filter.is_empty() {
                    button {
                        r#type: "button",
                        onclick: move |_| on_lot_filter_change.call(String::new()),
                        {t!("bidlog-filter-clear")}
                    }
                }
            }
            if bids.is_empty() {
                p { class: "public-auction-bid-log-empty", {t!("bidlog-filter-empty")} }
            } else {
            div { class: "public-auction-bid-log-table-wrap",
                table {
                    class: "public-auction-bid-log-table",
                    thead {
                        tr {
                            th { {t!("bidlog-time")} }
                            th { {t!("bidlog-lot")} }
                            th { {t!("bidlog-bidder")} }
                            th { class: "public-auction-bid-log-number", {t!("bidlog-amount")} }
                        }
                    }
                    tbody {
                        for bid in bids {
                            BidLogRow { bid }
                        }
                    }
                }
            }
            }
            PublicPager {
                page: current_page,
                page_size: page.page_size,
                total: page.total,
                has_more,
                label: t!("page-bidlog-title"),
                on_page_change,
            }
        }
    }
}

#[component]
fn BidLogRow(bid: BidLogEntry) -> Element {
    let lot_code = lot_code_for(&bid.equipment_category, bid.lot_number);
    let lot_id = bid.lot_id.clone();
    let validity = if bid.valid { t!("bidlog-valid") } else { t!("bidlog-invalid") };
    let marker = (bid.mechanism == "proxy").then(|| t!("catalog-proxy-marker"));
    rsx! {
        tr {
            class: if bid.valid { "public-auction-bid-log-leading" } else { "" },
            td { "data-label": t!("bidlog-time"),
                time { datetime: bid.created_at.clone(), "{format_auction_time(&bid.created_at)}" }
            }
            td { class: "public-auction-bid-log-code", "data-label": t!("bidlog-lot"),
                Link { to: Route::LotDetail { lot_code: lot_id }, "{lot_code}" }
            }
            td { class: "public-auction-bid-log-participant", "data-label": t!("bidlog-bidder"),
                CatalogParticipant {
                    name: bid.bidder_name,
                    marker,
                    trailing: None,
                }
                span { class: "public-visually-hidden", "{validity}" }
            }
            td {
                class: "public-auction-bid-log-amount public-auction-bid-log-number",
                "data-label": t!("bidlog-amount"),
                "{super::format_amount(bid.amount)}"
            }
        }
    }
}

fn bid_log_status_label(status: &str) -> String {
    match status {
        "live" => t!("auction-history-status-live"),
        "final" => t!("auction-history-status-final"),
        "settled" => t!("auction-history-status-settled"),
        _ => t!("bidlog-status-unknown"),
    }
}

fn bid_log_status_class(status: &str) -> &'static str {
    match status {
        "live" => "public-auction-bid-log-round-status public-auction-bid-log-round-status-live",
        "final" => "public-auction-bid-log-round-status public-auction-bid-log-round-status-final",
        _ => "public-auction-bid-log-round-status",
    }
}

pub(super) fn history() -> Element {
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
    let body = match &*history.read_unchecked() {
        None => rsx! {
            p { class: "public-auction-page__status", role: "status", {t!("data-loading")} }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("page-history-title"),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(page)) if page.rows.is_empty() => rsx! {
            Placeholder { title: t!("page-history-title"), detail: t!("data-empty") }
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
        section { class: "public-auction-page", aria_labelledby: "price-history-title",
            h1 { id: "price-history-title", class: "public-auction-category__title",
                {t!("page-history-title")}
            }
            p { class: "public-auction-page__status", {t!("page-history-note")} }
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
            {body}
        }
    }
}

#[component]
fn PriceHistoryFilters(
    mut world: Signal<String>,
    mut name: Signal<String>,
    mut from_month: Signal<String>,
    mut from_year: Signal<String>,
    mut to_month: Signal<String>,
    mut to_year: Signal<String>,
    mut minimum_price: Signal<String>,
    mut minimum_price_unit: Signal<HistoryPriceUnit>,
    mut maximum_price: Signal<String>,
    mut maximum_price_unit: Signal<HistoryPriceUnit>,
    mut buyer: Signal<String>,
    mut seller: Signal<String>,
    world_options: Vec<String>,
    active_filter_count: usize,
    mut settings_open: Signal<bool>,
    mut filter_error: Signal<Option<HistoryFilterError>>,
    on_apply: EventHandler<()>,
    on_reset: EventHandler<()>,
) -> Element {
    let current_error = filter_error();
    rsx! {
        form {
            class: "public-history-filters",
            onsubmit: move |event| {
                event.prevent_default();
                on_apply.call(());
            },
            fieldset {
                legend { {t!("history-filter-title")} }
                div { class: "public-history-primary-filters",
                    label {
                        span { {t!("history-filter-world")} }
                        select {
                            value: world(),
                            onchange: move |event| {
                                world.set(event.value());
                                filter_error.set(None);
                            },
                            option { value: "all", {t!("auction-history-world-all")} }
                            for option_world in world_options {
                                option { value: option_world.clone(), {super::world_label(&option_world)} }
                            }
                        }
                    }
                    label {
                        span { {t!("history-filter-name")} }
                        input {
                            r#type: "search",
                            value: name(),
                            placeholder: t!("history-filter-name-placeholder"),
                            maxlength: "1024",
                            oninput: move |event| {
                                name.set(event.value());
                                filter_error.set(None);
                            },
                        }
                    }
                    div { class: "public-history-filter-toolbar",
                        button {
                            r#type: "button",
                            class: "public-history-settings-toggle",
                            aria_controls: "public-history-settings",
                            aria_expanded: settings_open().to_string(),
                            onclick: move |_| settings_open.set(!settings_open()),
                            {t!("history-filter-settings")}
                            if active_filter_count > 0 {
                                span { class: "public-history-active-count",
                                    {t!("history-filter-settings-applied", count: active_filter_count)}
                                }
                            }
                        }
                        button {
                            r#type: "submit",
                            class: "public-history-search public-history-filter-actions__apply",
                            {t!("history-filter-apply")}
                        }
                    }
                }
                if settings_open() {
                    div {
                        id: "public-history-settings",
                        class: "public-history-settings-panel",
                        role: "group",
                        aria_label: t!("history-filter-settings"),
                        div { class: "public-history-settings-header",
                            strong { {t!("history-filter-settings")} }
                            button {
                                r#type: "button",
                                class: "public-history-settings-close",
                                aria_label: t!("history-filter-settings-close"),
                                onclick: move |_| settings_open.set(false),
                                {t!("history-filter-settings-close")}
                            }
                        }
                        div { class: "public-history-settings-grid",
                            HistoryDateFilter {
                                label: t!("history-filter-from"),
                                month_label: t!("history-filter-from-month"),
                                year_label: t!("history-filter-from-year"),
                                month: from_month,
                                year: from_year,
                                error: history_date_error(current_error, true),
                                filter_error,
                            }
                            HistoryDateFilter {
                                label: t!("history-filter-to"),
                                month_label: t!("history-filter-to-month"),
                                year_label: t!("history-filter-to-year"),
                                month: to_month,
                                year: to_year,
                                error: history_date_error(current_error, false),
                                filter_error,
                            }
                            HistoryPriceFilter {
                                label: t!("history-filter-min-price"),
                                value_label: t!("history-filter-price-value"),
                                unit_label: t!("history-filter-price-unit"),
                                price_value: minimum_price,
                                price_unit: minimum_price_unit,
                                error: history_price_error(current_error, true),
                                filter_error,
                            }
                            HistoryPriceFilter {
                                label: t!("history-filter-max-price"),
                                value_label: t!("history-filter-price-value"),
                                unit_label: t!("history-filter-price-unit"),
                                price_value: maximum_price,
                                price_unit: maximum_price_unit,
                                error: history_price_error(current_error, false),
                                filter_error,
                            }
                            label { class: "public-history-filter-cell",
                                span { {t!("history-filter-buyer")} }
                                input {
                                    r#type: "search",
                                    value: buyer(),
                                    maxlength: "256",
                                    oninput: move |event| {
                                        buyer.set(event.value());
                                        filter_error.set(None);
                                    },
                                }
                            }
                            label { class: "public-history-filter-cell",
                                span { {t!("history-filter-seller")} }
                                input {
                                    r#type: "search",
                                    value: seller(),
                                    maxlength: "256",
                                    oninput: move |event| {
                                        seller.set(event.value());
                                        filter_error.set(None);
                                    },
                                }
                            }
                        }
                        p { class: "public-history-price-hint", {t!("history-filter-price-hint")} }
                        div { class: "public-history-filter-actions",
                            button {
                                r#type: "button",
                                class: "public-history-filter-actions__reset",
                                onclick: move |_| on_reset.call(()),
                                {t!("history-filter-reset")}
                            }
                            button {
                                r#type: "button",
                                class: "public-history-filter-actions__clear",
                                onclick: move |_| {
                                    world.set("all".to_owned());
                                    name.set(String::new());
                                    from_month.set(String::new());
                                    from_year.set(String::new());
                                    to_month.set(String::new());
                                    to_year.set(String::new());
                                    minimum_price.set(String::new());
                                    minimum_price_unit.set(HistoryPriceUnit::K);
                                    maximum_price.set(String::new());
                                    maximum_price_unit.set(HistoryPriceUnit::K);
                                    buyer.set(String::new());
                                    seller.set(String::new());
                                    filter_error.set(None);
                                },
                                {t!("history-filter-clear")}
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HistoryPriceUnit {
    K,
    M,
}

impl HistoryPriceUnit {
    fn as_str(self) -> &'static str {
        match self {
            Self::K => "K",
            Self::M => "M",
        }
    }

    fn from_str(value: &str) -> Self {
        if value == "M" { Self::M } else { Self::K }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HistoryDateError {
    Incomplete,
    Invalid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HistoryPriceError {
    Invalid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HistoryFilterError {
    FromDateIncomplete,
    FromDateInvalid,
    ToDateIncomplete,
    ToDateInvalid,
    DateRange,
    MinimumPriceK,
    MinimumPriceM,
    MaximumPriceK,
    MaximumPriceM,
    PriceRange,
}

#[component]
fn HistoryDateFilter(
    label: String,
    month_label: String,
    year_label: String,
    mut month: Signal<String>,
    mut year: Signal<String>,
    error: Option<String>,
    mut filter_error: Signal<Option<HistoryFilterError>>,
) -> Element {
    rsx! {
        fieldset { class: "public-history-filter-cell",
            legend { {label} }
            div { class: "public-history-compound public-history-date",
                select {
                    aria_label: month_label,
                    aria_invalid: error.is_some().to_string(),
                    value: month(),
                    onchange: move |event| {
                        month.set(event.value());
                        filter_error.set(None);
                    },
                    option { value: "", {t!("history-filter-month")} }
                    for month_number in 1..=12 {
                        option {
                            value: format!("{month_number:02}"),
                            {history_month_label(month_number)}
                        }
                    }
                }
                input {
                    r#type: "number",
                    min: "1",
                    max: "9999",
                    inputmode: "numeric",
                    aria_label: year_label,
                    aria_invalid: error.is_some().to_string(),
                    placeholder: t!("history-filter-year"),
                    value: year(),
                    oninput: move |event| {
                        year.set(event.value());
                        filter_error.set(None);
                    },
                }
            }
            if let Some(ref error) = error {
                span { class: "public-history-field-error", role: "alert", {error.clone()} }
            }
        }
    }
}

#[component]
fn HistoryPriceFilter(
    label: String,
    value_label: String,
    unit_label: String,
    mut price_value: Signal<String>,
    mut price_unit: Signal<HistoryPriceUnit>,
    error: Option<String>,
    mut filter_error: Signal<Option<HistoryFilterError>>,
) -> Element {
    let input_mode = match price_unit() {
        HistoryPriceUnit::K => "numeric",
        HistoryPriceUnit::M => "decimal",
    };
    rsx! {
        fieldset { class: "public-history-filter-cell",
            legend { {label} }
            div { class: "public-history-compound public-history-price-control",
                input {
                    r#type: "text",
                    inputmode: input_mode,
                    maxlength: "32",
                    aria_label: value_label,
                    aria_invalid: error.is_some().to_string(),
                    value: price_value(),
                    oninput: move |event| {
                        price_value.set(event.value());
                        filter_error.set(None);
                    },
                }
                select {
                    aria_label: unit_label,
                    value: price_unit().as_str(),
                    onchange: move |event| {
                        price_unit.set(HistoryPriceUnit::from_str(&event.value()));
                        filter_error.set(None);
                    },
                    option { value: "K", "K" }
                    option { value: "M", "M" }
                }
            }
            if let Some(ref error) = error {
                span { class: "public-history-field-error", role: "alert", {error.clone()} }
            }
        }
    }
}

fn history_date_error(error: Option<HistoryFilterError>, from: bool) -> Option<String> {
    match (error?, from) {
        (HistoryFilterError::FromDateIncomplete, true)
        | (HistoryFilterError::ToDateIncomplete, false) => {
            Some(t!("history-filter-error-date-incomplete"))
        }
        (HistoryFilterError::FromDateInvalid, true)
        | (HistoryFilterError::ToDateInvalid, false) => {
            Some(t!("history-filter-error-date-invalid"))
        }
        (HistoryFilterError::DateRange, true) => Some(t!("history-filter-error-date-range")),
        _ => None,
    }
}

fn history_price_error(error: Option<HistoryFilterError>, minimum: bool) -> Option<String> {
    match (error?, minimum) {
        (HistoryFilterError::MinimumPriceK, true) | (HistoryFilterError::MaximumPriceK, false) => {
            Some(t!("history-filter-error-price-k"))
        }
        (HistoryFilterError::MinimumPriceM, true) | (HistoryFilterError::MaximumPriceM, false) => {
            Some(t!("history-filter-error-price-m"))
        }
        (HistoryFilterError::PriceRange, true) => Some(t!("history-filter-error-price-range")),
        _ => None,
    }
}

fn history_month_label(month: i32) -> String {
    match month {
        1 => t!("history-filter-month-01"),
        2 => t!("history-filter-month-02"),
        3 => t!("history-filter-month-03"),
        4 => t!("history-filter-month-04"),
        5 => t!("history-filter-month-05"),
        6 => t!("history-filter-month-06"),
        7 => t!("history-filter-month-07"),
        8 => t!("history-filter-month-08"),
        9 => t!("history-filter-month-09"),
        10 => t!("history-filter-month-10"),
        11 => t!("history-filter-month-11"),
        12 => t!("history-filter-month-12"),
        _ => month.to_string(),
    }
}

fn history_date_value(
    month: &str,
    year: &str,
    end: bool,
) -> Result<Option<String>, HistoryDateError> {
    let month = month.trim();
    let year = year.trim();
    if month.is_empty() && year.is_empty() {
        return Ok(None);
    }
    if month.is_empty() || year.is_empty() {
        return Err(HistoryDateError::Incomplete);
    }
    if year.len() != 4 || !year.chars().all(|value| value.is_ascii_digit()) {
        return Err(HistoryDateError::Invalid);
    }
    let Ok(year_number) = year.parse::<u32>() else {
        return Err(HistoryDateError::Invalid);
    };
    let Ok(month_number) = month.parse::<u32>() else {
        return Err(HistoryDateError::Invalid);
    };
    if year_number == 0 || !(1..=12).contains(&month_number) {
        return Err(HistoryDateError::Invalid);
    }
    let day = if end { history_days_in_month(year_number, month_number) } else { 1 };
    Ok(Some(format!("{year_number:04}-{month_number:02}-{day:02}")))
}

fn history_price_value(
    value: &str,
    unit: HistoryPriceUnit,
) -> Result<Option<String>, HistoryPriceError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let (whole, fraction) = match unit {
        HistoryPriceUnit::K => {
            if !value.chars().all(|character| character.is_ascii_digit()) {
                return Err(HistoryPriceError::Invalid);
            }
            (value, "")
        }
        HistoryPriceUnit::M => {
            let mut parts = value.split('.');
            let whole = parts.next().unwrap_or_default();
            let fraction = parts.next().unwrap_or_default();
            if parts.next().is_some()
                || whole.is_empty()
                || !whole.chars().all(|character| character.is_ascii_digit())
                || (fraction.is_empty() && value.contains('.'))
                || fraction.len() > 2
                || !fraction.chars().all(|character| character.is_ascii_digit())
            {
                return Err(HistoryPriceError::Invalid);
            }
            (whole, fraction)
        }
    };
    let Ok(whole) = whole.parse::<u64>() else {
        return Err(HistoryPriceError::Invalid);
    };
    let credits = match unit {
        HistoryPriceUnit::K => whole.checked_mul(1_000),
        HistoryPriceUnit::M => {
            let fraction_digits = fraction.len() as u32;
            let fraction = fraction.parse::<u64>().unwrap_or_default();
            whole
                .checked_mul(1_000_000)
                .and_then(|credits| credits.checked_add(fraction * 10u64.pow(4 - fraction_digits)))
        }
    };
    let Some(credits) = credits else {
        return Err(HistoryPriceError::Invalid);
    };
    if credits == 0 || credits > 9_007_199_254_740_991 {
        return Err(HistoryPriceError::Invalid);
    }
    Ok(Some(credits.to_string()))
}

fn normalize_history_query(mut query: super::PublicBoardQuery) -> super::PublicBoardQuery {
    query.history_from = query.history_from.filter(|value| history_is_valid_date(value));
    query.history_to = query.history_to.filter(|value| history_is_valid_date(value));
    query.history_min_price =
        query.history_min_price.and_then(|value| history_normalized_price(&value));
    query.history_max_price =
        query.history_max_price.and_then(|value| history_normalized_price(&value));
    if query
        .history_from
        .as_deref()
        .zip(query.history_to.as_deref())
        .is_some_and(|(from, to)| from > to)
    {
        query.history_from = None;
        query.history_to = None;
    }
    if query
        .history_min_price
        .as_deref()
        .zip(query.history_max_price.as_deref())
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        query.history_min_price = None;
        query.history_max_price = None;
    }
    query
}

fn history_date_parts(value: Option<&str>) -> (String, String) {
    let Some(value) = value.filter(|value| history_is_valid_date(value)) else {
        return (String::new(), String::new());
    };
    let mut parts = value.split('-');
    let year = parts.next().unwrap_or_default().to_owned();
    let month = parts.next().unwrap_or_default().to_owned();
    (month, year)
}

fn history_price_parts(value: Option<&str>) -> (String, HistoryPriceUnit) {
    let Some(value) = value.and_then(history_normalized_price) else {
        return (String::new(), HistoryPriceUnit::K);
    };
    let Ok(amount) = value.parse::<u64>() else {
        return (String::new(), HistoryPriceUnit::K);
    };
    if amount >= 1_000_000 && amount % 10_000 == 0 {
        ((amount / 1_000_000).to_string(), HistoryPriceUnit::M)
    } else {
        ((amount / 1_000).to_string(), HistoryPriceUnit::K)
    }
}

fn history_is_valid_date(value: &str) -> bool {
    let mut parts = value.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if year.len() != 4 || !year.chars().all(|value| value.is_ascii_digit()) {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) =
        (year.parse::<u32>(), month.parse::<u32>(), day.parse::<u32>())
    else {
        return false;
    };
    year > 0 && (1..=12).contains(&month) && (1..=history_days_in_month(year, month)).contains(&day)
}

fn history_normalized_price(value: &str) -> Option<String> {
    let amount = value.parse::<u64>().ok()?;
    (amount > 0 && amount <= 9_007_199_254_740_991 && amount % 1_000 == 0)
        .then(|| amount.to_string())
}

fn history_days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn history_active_filter_count(query: &PriceHistoryQuery) -> usize {
    [
        query.world.as_deref().filter(|value| *value != "all"),
        query.name.as_deref(),
        query.from.as_deref(),
        query.to.as_deref(),
        query.min_price.as_deref(),
        query.max_price.as_deref(),
        query.buyer.as_deref(),
        query.seller.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.is_empty())
    .count()
}

fn optional_filter_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[component]
fn PriceHistoryTable(
    page: PriceHistoryPage,
    current_page: i64,
    on_page_change: EventHandler<i64>,
) -> Element {
    let page_size = page.page_size.max(1);
    rsx! {
        div { class: "public-history-results",
            if let Some(stats) = page.stats.clone() {
                PriceHistoryStatsView { stats }
            }
            div { class: "public-auction-catalog public-history-table-wrap",
                table {
                    class: "public-history-table",
                    thead {
                        tr {
                            th { {t!("history-item")} }
                            th { {t!("history-category")} }
                            th { {t!("history-level")} }
                            th { {t!("history-world")} }
                            th { {t!("history-round")} }
                            th { {t!("history-price")} }
                            th { {t!("history-buyer")} }
                            th { {t!("history-seller")} }
                            th { {t!("history-closed")} }
                        }
                    }
                    tbody {
                        for row in page.rows {
                            PriceHistoryRow { row }
                        }
                    }
                }
            }
            PublicPager {
                page: current_page,
                page_size,
                total: page.total,
                has_more: page.has_more.unwrap_or(false),
                label: t!("catalog-pagination"),
                on_page_change,
            }
        }
    }
}

#[component]
fn PriceHistoryStatsView(stats: PriceHistoryStats) -> Element {
    rsx! {
        dl { class: "public-auction-preview-stats", aria_label: t!("history-statistics"),
            div {
                dt { {t!("history-count")} }
                dd { "{stats.count}" }
            }
            div {
                dt { {t!("history-min")} }
                dd { "{super::format_amount(stats.min)}" }
            }
            div {
                dt { {t!("history-max")} }
                dd { "{super::format_amount(stats.max)}" }
            }
            div {
                dt { {t!("history-median")} }
                dd { "{super::format_amount(stats.median)}" }
            }
        }
    }
}

#[component]
fn PriceHistoryRow(row: PriceHistoryEntry) -> Element {
    let lot_href = format!("/lots/{}", row.lot_id);
    let auction_href = history_auction_href(&row.auction_id, &row.world_id);
    let level =
        row.equipment_level.map(|level| level.to_string()).unwrap_or_else(|| t!("value-empty"));
    let world = super::world_label(&row.world_id);
    let category =
        super::catalog_category_label(super::catalog_category_key(&row.equipment_category));
    let price = super::format_amount(row.price);
    rsx! {
        tr {
            td { class: "public-history-table__item", "data-label": t!("history-item"),
                a { href: lot_href, title: row.equipment_name.clone(), "{row.equipment_name}" }
            }
            td { "data-label": t!("history-category"), "{category}" }
            td { class: "public-auction-catalog__number", "data-label": t!("history-level"), "{level}" }
            td { "data-label": t!("history-world"), "{world}" }
            td { class: "public-auction-catalog__number", "data-label": t!("history-round"),
                a { href: auction_href, "#{row.auction_sequence}" }
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("history-price"),
                "{price}"
            }
            td { "data-label": t!("history-buyer"), "{row.buyer_name}" }
            td { "data-label": t!("history-seller"), "{row.seller_name}" }
            td { "data-label": t!("history-closed"),
                time { datetime: row.closed_at.clone(), "{super::format_auction_time(&row.closed_at)}" }
            }
        }
    }
}

fn history_auction_href(auction_id: &str, world_id: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("auction", auction_id)
        .append_pair("world", world_id)
        .finish();
    format!("/auction?{query}")
}
