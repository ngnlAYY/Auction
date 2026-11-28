//! 分组接管运行参数的编辑器；未接管时明确显示继承状态，不伪造生效值。

mod alerts;
mod economics;
mod forum;
mod mail;
mod schedule;
mod worker;

use auction_web_shared::t;
use dioxus::prelude::*;

use super::{RuntimeSection, SwitchField};
use crate::surface::config::model::RuntimeConfigForm;

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
    // 任一分组开关只随该路径变化重绘，不订阅整份草稿的每次输入。
    let managed = use_memo(use_reactive((&path,), move |(path,)| form.read().boolean(path)));
    rsx! {
        RuntimeSection { title, open: false,
            SwitchField { form, disabled, path, label: t!("admin-runtime-op-managed"), on_dirty }
            if managed() {
                p { class: "admin-runtime-section-note",
                    {t!("admin-runtime-op-managed-note", hot_reload_note: t!("admin-runtime-hot-fields-note"))}
                }
                p { class: "admin-runtime-section-note", "{note}" }
                {children}
            } else {
                p { class: "admin-runtime-section-note", {t!("admin-runtime-op-inherited")} }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use dioxus_i18n::fluent::FluentArgs;
    use unic_langid::langid;

    use crate::surface::ADMIN_LOCALE_CATALOG;

    #[test]
    fn managed_group_note_uses_the_conditional_hot_reload_copy_in_every_locale() {
        let default_ftl = ADMIN_LOCALE_CATALOG.default_ftl;
        for (locale, ftl) in [
            (langid!("zh-CN"), default_ftl),
            (langid!("en"), include_str!("../../../../../i18n/en.ftl")),
            (langid!("zh-TW"), include_str!("../../../../../i18n/zh-TW.ftl")),
            (langid!("ja"), include_str!("../../../../../i18n/ja.ftl")),
            (langid!("ko"), include_str!("../../../../../i18n/ko.ftl")),
        ] {
            let bundle =
                build_bundle(&DEFAULT_ADMIN_LOCALE, default_ftl, &locale, Some(ftl)).unwrap();
            let format = |key, args: Option<&FluentArgs<'_>>| {
                let message = bundle.get_message(key).unwrap();
                let mut errors = Vec::new();
                let formatted =
                    bundle.format_pattern(message.value().unwrap(), args, &mut errors).into_owned();
                assert!(errors.is_empty(), "{locale}: {errors:?}");
                formatted
            };
            let conditional = format("admin-runtime-hot-fields-note", None);
            let mut args = FluentArgs::new();
            args.set("hot_reload_note", conditional.as_str());
            let managed = format("admin-runtime-op-managed-note", Some(&args));
            assert!(managed.contains(&conditional), "{locale}");
        }
    }
}
