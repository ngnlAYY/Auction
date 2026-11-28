//! 价格历史筛选器的 Dioxus 表单和字段控件。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::model::{HistoryField, HistoryPriceUnit};
use super::state::HistoryForm;
use super::validation::{history_error_message, history_month_label};
use crate::surface::world_label;

/// 渲染价格历史的主筛选表单。
#[component]
pub(in crate::surface::reads::history) fn PriceHistoryFilters(
    form: HistoryForm,
    world_options: Vec<String>,
    active_filter_count: usize,
) -> Element {
    let world = form.draft.read().world.clone();
    let world_error = form.error(HistoryField::World);
    let name_error = form.error(HistoryField::Name);
    let mut settings_open = form.settings_open;
    let mut settings_toggle = use_signal(|| None::<MountedEvent>);
    let close_settings = use_callback(move |()| {
        settings_open.set(false);
        if let Some(toggle) = settings_toggle() {
            spawn(async move {
                let _ = toggle.set_focus(true).await;
            });
        }
    });
    let name_description = if name_error.is_some() {
        "public-history-name-hint public-history-name-error"
    } else {
        "public-history-name-hint"
    };
    rsx! {
        form {
            class: "public-history-filters",
            action: "/history",
            method: "get",
            autocomplete: "off",
            novalidate: true,
            onsubmit: move |event| {
                event.prevent_default();
                form.apply();
            },
            div { class: "public-history-primary-filters",
                label { r#for: HistoryField::World.focus_id(),
                    span { id: "public-history-world-label", {t!("history-filter-world")} }
                    select {
                        id: HistoryField::World.focus_id(),
                        name: "world",
                        value: world,
                        aria_labelledby: "public-history-world-label",
                        aria_invalid: world_error.is_some().to_string(),
                        aria_describedby: world_error.as_ref().map(|_| "public-history-world-error"),
                        onchange: move |event| form.edit(HistoryField::World, |draft| draft.world = event.value()),
                        onblur: move |_| form.blur(HistoryField::World),
                        option { value: "all", {t!("auction-history-world-all")} }
                        if world != "all" && !world_options.contains(&world) {
                            option { value: world.clone(), disabled: true, selected: true,
                                if matches!(world.as_str(), "main" | "isekai") {
                                    {world_label(&world)}
                                } else {
                                    "{world}"
                                }
                            }
                        }
                        for option_world in world_options {
                            option { value: option_world, {world_label(&option_world)} }
                        }
                    }
                    FieldError { id: "public-history-world-error".to_owned(), error: world_error.clone() }
                }
                label { r#for: HistoryField::Name.focus_id(),
                    span { {t!("history-filter-name")} }
                    input {
                        id: HistoryField::Name.focus_id(),
                        name: "name",
                        r#type: "search",
                        enterkeyhint: "search",
                        autocapitalize: "none",
                        spellcheck: "false",
                        value: form.draft.read().name.clone(),
                        placeholder: t!("history-filter-name-placeholder"),
                        maxlength: "1024",
                        aria_invalid: name_error.is_some().to_string(),
                        aria_describedby: name_description,
                        oninput: move |event| form.edit(HistoryField::Name, |draft| draft.name = event.value()),
                        onblur: move |_| form.blur(HistoryField::Name),
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
            FieldError { id: "public-history-name-error".to_owned(), error: name_error.clone() }
            span { class: "ui-pager__sr-only", aria_live: "polite", aria_atomic: "true",
                if let Some(error) = (form.announcement)() { {history_error_message(error)} }
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
                            form, field: HistoryField::From,
                            label: t!("history-filter-from"),
                            month_label: t!("history-filter-from-month"),
                            year_label: t!("history-filter-from-year"),
                        }
                        HistoryDateFilter {
                            form, field: HistoryField::To,
                            label: t!("history-filter-to"),
                            month_label: t!("history-filter-to-month"),
                            year_label: t!("history-filter-to-year"),
                        }
                        HistoryPriceFilter {
                            form, field: HistoryField::Minimum,
                            label: t!("history-filter-min-price"),
                            value_label: t!("history-filter-min-price-value"),
                            unit_label: t!("history-filter-min-price-unit"),
                        }
                        HistoryPriceFilter {
                            form, field: HistoryField::Maximum,
                            label: t!("history-filter-max-price"),
                            value_label: t!("history-filter-max-price-value"),
                            unit_label: t!("history-filter-max-price-unit"),
                        }
                        HistoryPlayerFilter { form, field: HistoryField::Buyer, label: t!("history-filter-buyer") }
                        HistoryPlayerFilter { form, field: HistoryField::Seller, label: t!("history-filter-seller") }
                    }
                    p { class: "public-history-price-hint", {t!("history-filter-price-hint")} }
                    p { class: "public-history-player-hint", {t!("history-filter-player-hint")} }
                    div { class: "public-history-filter-actions",
                        button {
                            r#type: "button",
                            class: "public-history-filter-actions__clear",
                            onclick: move |_| form.reset(),
                            {t!("history-filter-clear")}
                        }
                        crate::surface::presentation::mobile::HistoryApply {}
                    }
                }
            }
        }
    }
}

#[component]
fn FieldError(id: String, error: Option<String>) -> Element {
    rsx! {
        if let Some(error) = error {
            span { id, class: "public-history-field-error", "{error}" }
        }
    }
}

#[component]
fn HistoryDateFilter(
    form: HistoryForm,
    field: HistoryField,
    label: String,
    month_label: String,
    year_label: String,
) -> Element {
    let field_name = field.key();
    let error = form.error(field);
    let error_id = format!("public-history-{field_name}-error");
    let from = field == HistoryField::From;
    let date = if from { form.draft.read().from.clone() } else { form.draft.read().to.clone() };
    rsx! {
        fieldset { class: "public-history-filter-cell",
            legend { {label} }
            div { class: "public-history-compound public-history-date",
                select {
                    id: field.month_focus_id(),
                    name: "{field_name}_month",
                    aria_describedby: error.as_ref().map(|_| error_id.clone()),
                    aria_label: month_label,
                    aria_invalid: error.is_some().to_string(),
                    value: date.month,
                    onchange: move |event| form.edit(field, |draft| {
                        let date = if from { &mut draft.from } else { &mut draft.to };
                        date.month = event.value();
                    }),
                    onblur: move |_| form.blur(field),
                    option { value: "", {t!("history-filter-month")} }
                    for month_number in 1..=12 {
                        option {
                            value: format!("{month_number:02}"),
                            {history_month_label(month_number)}
                        }
                    }
                }
                input {
                    id: field.focus_id(),
                    name: "{field_name}_year",
                    aria_describedby: error.as_ref().map(|_| error_id.clone()),
                    r#type: "text",
                    inputmode: "numeric",
                    pattern: "[0-9]{4}",
                    aria_label: year_label,
                    aria_invalid: error.is_some().to_string(),
                    placeholder: t!("history-filter-year"),
                    value: date.year,
                    oninput: move |event| form.edit(field, |draft| {
                        let date = if from { &mut draft.from } else { &mut draft.to };
                        date.year = event.value();
                    }),
                    onblur: move |_| form.blur(field),
                }
            }
            FieldError { id: error_id.clone(), error: error.clone() }
        }
    }
}

#[component]
fn HistoryPriceFilter(
    form: HistoryForm,
    field: HistoryField,
    label: String,
    value_label: String,
    unit_label: String,
) -> Element {
    let minimum = field == HistoryField::Minimum;
    let field_name = if minimum { "minimum_price" } else { "maximum_price" };
    let error = form.error(field);
    let error_id = format!("public-history-{field_name}-error");
    let price =
        if minimum { form.draft.read().minimum.clone() } else { form.draft.read().maximum.clone() };
    let input_mode = match price.unit {
        HistoryPriceUnit::C => "numeric",
        HistoryPriceUnit::K | HistoryPriceUnit::M => "decimal",
    };
    rsx! {
        fieldset { class: "public-history-filter-cell",
            legend { {label} }
            div { class: "public-history-compound public-history-price-control",
                input {
                    id: field.focus_id(),
                    name: field_name,
                    aria_describedby: error.as_ref().map(|_| error_id.clone()),
                    r#type: "text",
                    inputmode: input_mode,
                    maxlength: "32",
                    aria_label: value_label,
                    aria_invalid: error.is_some().to_string(),
                    value: price.value,
                    oninput: move |event| form.edit(field, |draft| {
                        let price = if minimum { &mut draft.minimum } else { &mut draft.maximum };
                        price.value = event.value();
                    }),
                    onblur: move |_| form.blur(field),
                }
                select {
                    name: "{field_name}_unit",
                    aria_describedby: error.as_ref().map(|_| error_id.clone()),
                    aria_label: unit_label,
                    aria_invalid: error.is_some().to_string(),
                    value: price.unit.as_str(),
                    onchange: move |event| form.edit(field, |draft| {
                        let price = if minimum { &mut draft.minimum } else { &mut draft.maximum };
                        price.unit = HistoryPriceUnit::from_str(&event.value());
                    }),
                    onblur: move |_| form.blur(field),
                    option { value: "C", "C" }
                    option { value: "K", "K" }
                    option { value: "M", "M" }
                }
            }
            FieldError { id: error_id.clone(), error: error.clone() }
        }
    }
}

#[component]
fn HistoryPlayerFilter(form: HistoryForm, field: HistoryField, label: String) -> Element {
    let error = form.error(field);
    let id = field.focus_id();
    let error_id = format!("{id}-error");
    let label_id = format!("{id}-label");
    let buyer = field == HistoryField::Buyer;
    let draft = form.draft.read();
    let name = if buyer { draft.buyer.clone() } else { draft.seller.clone() };
    rsx! {
        label { class: "public-history-filter-cell", r#for: id,
            span { id: label_id.clone(), "{label}" }
            input {
                id, name: field.key(),
                r#type: "search",
                enterkeyhint: "search",
                autocapitalize: "none",
                spellcheck: "false",
                value: name,
                maxlength: "256",
                aria_labelledby: label_id,
                aria_invalid: error.is_some().to_string(),
                aria_describedby: error.as_ref().map(|_| error_id.clone()),
                oninput: move |event| form.edit(field, |draft| {
                    if buyer { draft.buyer = event.value(); } else { draft.seller = event.value(); }
                }),
                onblur: move |_| form.blur(field),
            }
            FieldError { id: error_id.clone(), error: error.clone() }
        }
    }
}

#[cfg(test)]
mod tests {
    use dioxus::prelude::*;

    use super::super::state::use_history_form;
    use super::PriceHistoryFilters;
    use crate::surface::{
        PageBrowserFixture, assert_selected_error, page_test_contexts, use_page_query,
    };

    fn filters() -> Element {
        page_test_contexts();
        let state = use_page_query();
        let form = use_history_form(state);
        rsx! {
            PriceHistoryFilters { form, world_options: vec!["main".to_owned(), "isekai".to_owned()], active_filter_count: 1 }
        }
    }

    #[test]
    fn history_first_frame_selects_and_describes_the_unknown_world() {
        let browser = PageBrowserFixture::new("/history", "world=outside%20world");
        let mut dom = VirtualDom::new(filters).with_root_context(browser);
        let initial = dom.rebuild_to_vec();
        assert_selected_error(
            &dom,
            &initial.edits,
            "outside world",
            "public-history-world-error",
            "Select a world or All worlds.",
        );
    }
}
