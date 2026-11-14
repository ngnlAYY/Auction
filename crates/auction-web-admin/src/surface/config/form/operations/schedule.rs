//! 自动周排期与赛季早鸟期参数编辑控件。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::{
    ListField, ManagedSection, NumberField, RuntimeConfigForm, RuntimeGroup, SelectField,
    SwitchField, TextField,
};

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
            EarlyBirdFields { form, disabled, on_dirty }
        }
    }
}

/// 赛季早鸟期：赛季前场次与窗口内场次替代重叠的常规周场、使用单独品质并在赛季前暂停收件。
#[component]
fn EarlyBirdFields(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    rsx! {
        RuntimeGroup { title: t!("admin-runtime-op-early-bird"),
            p { class: "admin-runtime-section-note", {t!("admin-runtime-op-early-bird-note")} }
            div { class: "admin-runtime-field-grid",
                SwitchField { form, disabled, path: "operations.schedule.early_bird.enabled", label: t!("admin-runtime-op-enabled"), on_dirty }
                TextField { form, disabled, path: "operations.schedule.early_bird.timezone", label: t!("admin-runtime-op-timezone"), on_dirty }
                ListField { form, disabled, path: "operations.schedule.early_bird.worlds", label: t!("admin-runtime-op-worlds"), on_dirty }
                ListField { form, disabled, path: "operations.schedule.early_bird.season_starts", label: t!("admin-runtime-op-early-bird-season-starts"), on_dirty }
                TimeField { form, disabled, path: "operations.schedule.early_bird.season_start_time", label: t!("admin-runtime-op-early-bird-season-start-time"), on_dirty }
                NumberField { form, disabled, path: "operations.schedule.early_bird.window_days", label: t!("admin-runtime-op-early-bird-window-days"), minimum: "1", maximum: "31", on_dirty }
                ListField { form, disabled, path: "operations.schedule.early_bird.session_days", label: t!("admin-runtime-op-early-bird-session-days"), on_dirty }
                TimeField { form, disabled, path: "operations.schedule.early_bird.open_time", label: t!("admin-runtime-op-open-time"), on_dirty }
                NumberField { form, disabled, path: "operations.schedule.early_bird.preview_lead_minutes", label: t!("admin-runtime-op-early-bird-preview-lead"), minimum: "120", maximum: "10080", on_dirty }
                NumberField { form, disabled, path: "operations.schedule.early_bird.live_minutes", label: t!("admin-runtime-op-early-bird-live-minutes"), minimum: "1", maximum: "10080", on_dirty }
                SwitchField { form, disabled, path: "operations.schedule.early_bird.pre_season_session", label: t!("admin-runtime-op-early-bird-pre-season"), on_dirty }
                NumberField { form, disabled, path: "operations.schedule.early_bird.pre_season_days_before", label: t!("admin-runtime-op-early-bird-pre-season-days"), minimum: "1", maximum: "31", on_dirty }
                ListField { form, disabled, path: "operations.schedule.early_bird.qualities", label: t!("admin-runtime-op-early-bird-qualities"), on_dirty }
                SwitchField { form, disabled, path: "operations.schedule.early_bird.pause_intake_before_season", label: t!("admin-runtime-op-early-bird-pause-intake"), on_dirty }
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
