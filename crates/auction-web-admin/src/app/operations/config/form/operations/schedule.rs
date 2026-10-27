//! 自动周排期参数编辑控件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::{ListField, ManagedSection, RuntimeConfigForm, SelectField, SwitchField, TextField};

#[component]
pub(super) fn ScheduleFields(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    rsx! {
        ManagedSection { form, disabled, path: "operations.schedule.managed", title: t!("admin-runtime-op-section-schedule"), note: t!("admin-runtime-op-note-schedule"), on_dirty,
            div { class: "admin-runtime-field-grid",
                SwitchField { form, disabled, path: "operations.schedule.enabled", label: t!("admin-runtime-op-enabled"), on_dirty }
                TextField { form, disabled, path: "operations.schedule.timezone", label: t!("admin-runtime-op-timezone"), on_dirty }
                SelectField { form, disabled, path: "operations.schedule.preview_day", label: t!("admin-runtime-op-preview-day"), options: weekday_options(), on_dirty }
                TimeField { form, disabled, path: "operations.schedule.preview_time", label: t!("admin-runtime-op-preview-time"), on_dirty }
                SelectField { form, disabled, path: "operations.schedule.open_day", label: t!("admin-runtime-op-open-day"), options: weekday_options(), on_dirty }
                TimeField { form, disabled, path: "operations.schedule.open_time", label: t!("admin-runtime-op-open-time"), on_dirty }
                SelectField { form, disabled, path: "operations.schedule.nominal_end_day", label: t!("admin-runtime-op-nominal-end-day"), options: weekday_options(), on_dirty }
                TimeField { form, disabled, path: "operations.schedule.nominal_end_time", label: t!("admin-runtime-op-nominal-end-time"), on_dirty }
                ListField { form, disabled, path: "operations.schedule.worlds", label: t!("admin-runtime-op-worlds"), on_dirty }
            }
        }
    }
}

fn weekday_options() -> Vec<(String, String)> {
    vec![
        ("Monday".to_owned(), t!("admin-runtime-op-monday")),
        ("Tuesday".to_owned(), t!("admin-runtime-op-tuesday")),
        ("Wednesday".to_owned(), t!("admin-runtime-op-wednesday")),
        ("Thursday".to_owned(), t!("admin-runtime-op-thursday")),
        ("Friday".to_owned(), t!("admin-runtime-op-friday")),
        ("Saturday".to_owned(), t!("admin-runtime-op-saturday")),
        ("Sunday".to_owned(), t!("admin-runtime-op-sunday")),
    ]
}

#[component]
fn TimeField(
    mut form: Signal<RuntimeConfigForm>,
    disabled: bool,
    path: &'static str,
    label: String,
    on_dirty: EventHandler<()>,
) -> Element {
    let value = form.read().input(path);
    rsx! {
        label { class: "admin-runtime-field",
            span { "{label}" }
            input { r#type: "time", step: "60", disabled, value,
                name: path,
                autocomplete: "off",
                oninput: move |event: FormEvent| {
                    form.write().set_input(path, event.value());
                    on_dirty.call(());
                },
            }
        }
    }
}
