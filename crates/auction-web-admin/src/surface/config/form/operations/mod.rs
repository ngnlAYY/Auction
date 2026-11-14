//! 分组接管运行参数的编辑器；未接管时明确显示继承状态，不伪造生效值。

mod alerts;
mod economics;
mod forum;
mod mail;
mod schedule;
mod worker;

use auction_web_shared::t;
use dioxus::prelude::*;

use super::{
    ListField, NumberField, RuntimeConfigForm, RuntimeGroup, RuntimeSection, SelectField,
    SwitchField, TextField,
};

#[component]
pub(super) fn OperationalConfigFields(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    rsx! {
        p { class: "admin-runtime-section-warning", {t!("admin-runtime-hot-fields-note")} }
        mail::MailFields { form, disabled, on_dirty }
        forum::ForumFields { form, disabled, on_dirty }
        economics::EconomicsFields { form, disabled, on_dirty }
        schedule::ScheduleFields { form, disabled, on_dirty }
        alerts::AlertsFields { form, disabled, on_dirty }
        worker::WorkerFields { form, disabled, on_dirty }
    }
}

#[component]
fn ManagedSection(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    path: &'static str,
    title: String,
    note: String,
    on_dirty: EventHandler<()>,
    children: Element,
) -> Element {
    let managed = form.read().boolean(path);
    rsx! {
        RuntimeSection { title, open: false,
            SwitchField { form, disabled, path, label: t!("admin-runtime-op-managed"), on_dirty }
            if managed {
                p { class: "admin-runtime-section-note", {t!("admin-runtime-op-managed-note")} }
                p { class: "admin-runtime-section-note", "{note}" }
                {children}
            } else {
                p { class: "admin-runtime-section-note", {t!("admin-runtime-op-inherited")} }
            }
        }
    }
}
