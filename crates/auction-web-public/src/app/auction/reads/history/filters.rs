use super::*;

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
pub(crate) enum HistoryPriceUnit {
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
pub(crate) enum HistoryDateError {
    Incomplete,
    Invalid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryPriceError {
    Invalid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryFilterError {
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
pub(crate) fn HistoryDateFilter(
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
pub(crate) fn HistoryPriceFilter(
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

pub(crate) fn history_date_error(error: Option<HistoryFilterError>, from: bool) -> Option<String> {
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

pub(crate) fn history_price_error(
    error: Option<HistoryFilterError>,
    minimum: bool,
) -> Option<String> {
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

pub(crate) fn history_month_label(month: i32) -> String {
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

pub(crate) fn history_date_value(
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

pub(crate) fn history_price_value(
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
