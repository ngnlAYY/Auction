//! 价格历史筛选器的 Dioxus 表单和字段控件。

use super::super::world_label;
use super::{
    HistoryFilterError, HistoryPriceUnit, history_date_error, history_month_label,
    history_name_error, history_name_value, history_price_error,
};
use auction_web_shared::t;
use dioxus::prelude::*;

/// 渲染价格历史的主筛选表单。
#[component]
pub(crate) fn PriceHistoryFilters(
    mut world: Signal<String>,
    mut name: Signal<String>,
    from_month: Signal<String>,
    from_year: Signal<String>,
    to_month: Signal<String>,
    to_year: Signal<String>,
    minimum_price: Signal<String>,
    minimum_price_unit: Signal<HistoryPriceUnit>,
    maximum_price: Signal<String>,
    maximum_price_unit: Signal<HistoryPriceUnit>,
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
    let mut settings_toggle = use_signal(|| None::<MountedEvent>);
    let close_settings = use_callback(move |()| {
        settings_open.set(false);
        if let Some(toggle) = settings_toggle() {
            spawn(async move {
                let _ = toggle.set_focus(true).await;
            });
        }
    });
    let name_error = history_name_error(current_error);
    let name_description = if name_error.is_some() {
        "public-history-name-hint public-history-name-error"
    } else {
        "public-history-name-hint"
    };
    rsx! {
        form {
            class: "public-history-filters",
            autocomplete: "off",
            onsubmit: move |event| {
                event.prevent_default();
                on_apply.call(());
            },
            div { class: "public-history-primary-filters",
                    label {
                        span { {t!("history-filter-world")} }
                        select {
                            name: "world",
                            value: world(),
                            onchange: move |event| {
                                world.set(event.value());
                                filter_error.set(None);
                            },
                            option { value: "all", {t!("auction-history-world-all")} }
                            for option_world in world_options {
                                option { value: option_world, {world_label(&option_world)} }
                            }
                        }
                    }
                    label { r#for: "public-history-name",
                        span { {t!("history-filter-name")} }
                        input {
                            id: "public-history-name",
                            name: "name",
                            r#type: "search",
                            enterkeyhint: "search",
                            autocapitalize: "none",
                            spellcheck: "false",
                            value: name(),
                            placeholder: t!("history-filter-name-placeholder"),
                            maxlength: "1024",
                            aria_invalid: name_error.is_some().to_string(),
                            aria_describedby: name_description,
                            oninput: move |event| {
                                name.set(event.value());
                                filter_error.set(None);
                            },
                            onblur: move |_| {
                                let error = history_name_value(&name()).err();
                                if error.is_some() || history_name_error(filter_error()).is_some() {
                                    filter_error.set(error);
                                }
                            },
                        }
                    }
                    div { class: "public-history-filter-toolbar",
                        button {
                            r#type: "button",
                            class: "public-history-settings-toggle",
                            aria_controls: "public-history-settings",
                            aria_expanded: settings_open().to_string(),
                            onmounted: move |event| settings_toggle.set(Some(event)),
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
                p { id: "public-history-name-hint", class: "public-history-name-hint",
                    {t!("history-filter-name-hint")}
                }
                if let Some(ref error) = name_error {
                    p { id: "public-history-name-error", class: "public-history-field-error", role: "alert", {error.clone()} }
                }
                if settings_open() {
                    div {
                        id: "public-history-settings",
                        class: "public-history-settings-panel",
                        role: "group",
                        aria_label: t!("history-filter-settings"),
                        onkeydown: move |event| {
                            if event.key() == Key::Escape {
                                event.prevent_default();
                                event.stop_propagation();
                                close_settings.call(());
                            }
                        },
                        div { class: "public-history-settings-header",
                            strong { {t!("history-filter-settings")} }
                            button {
                                r#type: "button",
                                class: "public-history-settings-close",
                                aria_label: t!("history-filter-settings-close"),
                                onclick: move |_| close_settings.call(()),
                                span { aria_hidden: "true", "×" }
                            }
                        }
                        div { class: "public-history-settings-grid",
                            HistoryDateFilter {
                                field_name: "from",
                                label: t!("history-filter-from"),
                                month_label: t!("history-filter-from-month"),
                                year_label: t!("history-filter-from-year"),
                                month: from_month,
                                year: from_year,
                                error: history_date_error(current_error, true),
                                filter_error,
                            }
                            HistoryDateFilter {
                                field_name: "to",
                                label: t!("history-filter-to"),
                                month_label: t!("history-filter-to-month"),
                                year_label: t!("history-filter-to-year"),
                                month: to_month,
                                year: to_year,
                                error: history_date_error(current_error, false),
                                filter_error,
                            }
                            HistoryPriceFilter {
                                field_name: "minimum_price",
                                label: t!("history-filter-min-price"),
                                value_label: t!("history-filter-min-price-value"),
                                unit_label: t!("history-filter-min-price-unit"),
                                price_value: minimum_price,
                                price_unit: minimum_price_unit,
                                error: history_price_error(current_error, true),
                                filter_error,
                            }
                            HistoryPriceFilter {
                                field_name: "maximum_price",
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
                                    name: "buyer",
                                    r#type: "search",
                                    enterkeyhint: "search",
                                    autocapitalize: "none",
                                    spellcheck: "false",
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
                                    name: "seller",
                                    r#type: "search",
                                    enterkeyhint: "search",
                                    autocapitalize: "none",
                                    spellcheck: "false",
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
                        p { class: "public-history-player-hint", {t!("history-filter-player-hint")} }
                        div { class: "public-history-filter-actions",
                            button {
                                r#type: "button",
                                class: "public-history-filter-actions__clear",
                                // 同步清除已应用的 URL 条件，保留顶部搜索并移除旧结果。
                                onclick: move |_| on_reset.call(()),
                                {t!("history-filter-clear")}
                            }
                            crate::surface::presentation::mobile::HistoryApply {}
                        }
                    }
            }
        }
    }
}

/// 渲染一个年月组合筛选字段。
#[component]
fn HistoryDateFilter(
    field_name: &'static str,
    label: String,
    month_label: String,
    year_label: String,
    mut month: Signal<String>,
    mut year: Signal<String>,
    error: Option<String>,
    mut filter_error: Signal<Option<HistoryFilterError>>,
) -> Element {
    let error_id = format!("public-history-{field_name}-error");
    rsx! {
        fieldset { class: "public-history-filter-cell",
            legend { {label} }
            div { class: "public-history-compound public-history-date",
                select {
                    name: "{field_name}_month",
                    aria_describedby: error.as_ref().map(|_| error_id.clone()),
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
                    name: "{field_name}_year",
                    aria_describedby: error.as_ref().map(|_| error_id.clone()),
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
                span { id: error_id.as_str(), class: "public-history-field-error", role: "alert", {error.clone()} }
            }
        }
    }
}

/// 渲染一个带 K/M 单位选择的价格字段。
#[component]
fn HistoryPriceFilter(
    field_name: &'static str,
    label: String,
    value_label: String,
    unit_label: String,
    mut price_value: Signal<String>,
    mut price_unit: Signal<HistoryPriceUnit>,
    error: Option<String>,
    mut filter_error: Signal<Option<HistoryFilterError>>,
) -> Element {
    let error_id = format!("public-history-{field_name}-error");
    let input_mode = match price_unit() {
        HistoryPriceUnit::C => "numeric",
        HistoryPriceUnit::K | HistoryPriceUnit::M => "decimal",
    };
    rsx! {
        fieldset { class: "public-history-filter-cell",
            legend { {label} }
            div { class: "public-history-compound public-history-price-control",
                input {
                    name: field_name,
                    aria_describedby: error.as_ref().map(|_| error_id.clone()),
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
                    name: "{field_name}_unit",
                    aria_describedby: error.as_ref().map(|_| error_id.clone()),
                    aria_label: unit_label,
                    value: price_unit().as_str(),
                    onchange: move |event| {
                        price_unit.set(HistoryPriceUnit::from_str(&event.value()));
                        filter_error.set(None);
                    },
                    option { value: "C", "C" }
                    option { value: "K", "K" }
                    option { value: "M", "M" }
                }
            }
            if let Some(ref error) = error {
                span { id: error_id.as_str(), class: "public-history-field-error", role: "alert", {error.clone()} }
            }
        }
    }
}
