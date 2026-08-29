//! 价格历史筛选器的 Dioxus 表单和字段控件。

use super::super::world_label;
use super::{
    HistoryFilterError, HistoryPriceUnit, history_date_error, history_month_label,
    history_price_error,
};
use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

/// 渲染价格历史的主筛选表单。
#[component]
pub(crate) fn PriceHistoryFilters(
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
                                option { value: option_world.clone(), {world_label(&option_world)} }
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
                            span { aria_hidden: "true", "⚙" }
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
                                span { aria_hidden: "true", "×" }
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
                                value_label: t!("history-filter-min-price-value"),
                                unit_label: t!("history-filter-min-price-unit"),
                                price_value: minimum_price,
                                price_unit: minimum_price_unit,
                                error: history_price_error(current_error, true),
                                filter_error,
                            }
                            HistoryPriceFilter {
                                label: t!("history-filter-max-price"),
                                value_label: t!("history-filter-max-price-value"),
                                unit_label: t!("history-filter-max-price-unit"),
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

/// 渲染一个年月组合筛选字段。
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

/// 渲染一个带 K/M 单位选择的价格字段。
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
