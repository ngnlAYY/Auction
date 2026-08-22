//! Admin 规则文档页面。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{AdminRules, fetch_admin_rules, publish_admin_rules, validate_admin_rules};

use super::AdminPanel;

/// 加载并展示当前英文规则文档；服务端负责其他语言回退和版本校验。
pub(super) fn page() -> Element {
    let reload_generation = use_signal(|| 0_u64);
    let rules = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_rules()
    });

    match &*rules.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-rules-title"), detail: t!("admin-rules-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminPanel { title: t!("page-rules-title"), detail: t!("admin-rules-error") }
        },
        Some(Ok(document)) => rsx! {
            AdminRulesEditor { document: document.clone(), reload_generation }
        },
    }
}

#[component]
fn AdminRulesEditor(document: AdminRules, reload_generation: Signal<u64>) -> Element {
    let mut locale = use_signal(|| document.locale.clone());
    let mut title = use_signal(|| document.title.clone());
    let mut markdown = use_signal(|| document.markdown.clone());
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<bool>);

    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-rules-title",
            h1 { id: "admin-rules-title", {t!("page-rules-title")} }
            p { class: "admin-panel__note", {t!("admin-rules-revision", revision: document.revision)} }
            p { class: "admin-panel__note", {t!("admin-rules-published", actor: document.published_by.clone())} }
            div { class: "admin-panel__actions",
                label {
                    {t!("admin-rules-locale")}
                    input {
                        value: locale(),
                        oninput: move |event| locale.set(event.value()),
                    }
                }
                label {
                    {t!("admin-rules-title")}
                    input {
                        value: title(),
                        oninput: move |event| title.set(event.value()),
                    }
                }
                label { class: "admin-rules__markdown",
                    {t!("admin-rules-markdown")}
                    textarea {
                        value: markdown(),
                        rows: "18",
                        oninput: move |event| markdown.set(event.value()),
                    }
                }
                div { class: "admin-panel__actions",
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| {
                            let locale = locale();
                            let title = title();
                            let markdown = markdown();
                            let mut busy = busy;
                            let mut feedback = feedback;
                            spawn(async move {
                                busy.set(true);
                                let result = validate_admin_rules(locale, title, markdown).await;
                                feedback.set(Some(result.is_ok()));
                                busy.set(false);
                            });
                        },
                        {t!("admin-rules-validate")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| {
                            let expected_revision = document.revision;
                            let locale = locale();
                            let title = title();
                            let markdown = markdown();
                            let mut busy = busy;
                            let mut feedback = feedback;
                            let mut reload_generation = reload_generation;
                            spawn(async move {
                                busy.set(true);
                                let result = publish_admin_rules(
                                    expected_revision,
                                    locale,
                                    title,
                                    markdown,
                                )
                                .await;
                                match result {
                                    Ok(_) => {
                                        feedback.set(Some(true));
                                        reload_generation.set(reload_generation().saturating_add(1));
                                    }
                                    Err(_) => feedback.set(Some(false)),
                                }
                                busy.set(false);
                            });
                        },
                        {t!("admin-rules-publish")}
                    }
                }
            }
            if let Some(success) = feedback() {
                p { class: "admin-panel__note",
                    {if success {
                        t!("admin-rules-success")
                    } else {
                        t!("admin-rules-action-error")
                    }}
                }
            }
        }
    }
}
