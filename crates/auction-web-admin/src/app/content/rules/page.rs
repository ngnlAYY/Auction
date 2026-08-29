//! 规则页面加载、语言切换和 workspace 装配。

use auction_web_shared::SUPPORTED_LOCALES;
use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::fetch_admin_rules;
use crate::app::surface::admin_api_error_message;

use super::helpers::{DEFAULT_RULES_LOCALE, missing_rules_document, to_draft};
use super::model::{RuleCommand, RuleNotice};
use super::workspace::AdminRulesWorkspace;

/// 读取并展示 Admin 规则文档；语言切换会让请求带上旧版约定的 `lang` 查询参数。
pub fn page() -> Element {
    let target = use_signal(|| DEFAULT_RULES_LOCALE.to_owned());
    let locales = use_signal(|| {
        SUPPORTED_LOCALES.iter().map(|locale| locale.code.to_owned()).collect::<Vec<_>>()
    });
    let new_locale = use_signal(String::new);
    let reload_generation = use_signal(|| 0_u64);
    let mut draft = use_signal(|| None::<crate::api::AdminRulesCandidate>);
    let mut checked_signature = use_signal(String::new);
    let mut command = use_signal(|| None::<RuleCommand>);
    let mut notice = use_signal(|| None::<RuleNotice>);
    let busy = use_signal(|| false);
    let rules = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_rules(target())
    });

    // 规则请求成功后才初始化编辑器，避免用户编辑过程中普通重绘把草稿覆盖掉。
    use_effect(move || {
        let requested_locale = target();
        let snapshot = rules.read_unchecked().clone();
        if let Some(Ok(document)) = snapshot {
            draft.set(Some(to_draft(&document, &requested_locale)));
            checked_signature.set(String::new());
            command.set(None);
            notice.set(None);
        }
    });

    let content = match &*rules.read_unchecked() {
        None => rsx! {
            p { class: "admin-rules-status", role: "status", {t!("admin-rules-loading")} }
        },
        Some(Err(error)) if error.status == Some(404) => {
            let document = missing_rules_document(target());
            rsx! {
                p { class: "admin-rules-status", role: "status", {t!("admin-rules-missing")} }
                AdminRulesWorkspace {
                    document,
                    target: target(),
                    locales,
                    new_locale,
                    target_signal: target,
                    draft,
                    checked_signature,
                    command,
                    notice,
                    busy,
                    reload_generation,
                }
            }
        }
        Some(Err(error)) => rsx! {
            AdminRulesLoadError {
                detail: admin_api_error_message(&error.code),
                reload_generation,
            }
        },
        Some(Ok(document)) => rsx! {
            AdminRulesWorkspace {
                document: document.clone(),
                target: target(),
                locales,
                new_locale,
                target_signal: target,
                draft,
                checked_signature,
                command,
                notice,
                busy,
                reload_generation,
            }
        },
    };

    rsx! {
        section {
            class: "admin-rules-page",
            aria_labelledby: "rules-title",
            h1 { id: "rules-title", {t!("page-rules-title")} }
            p { class: "admin-rules-intro", {t!("admin-rules-description")} }
            {content}
        }
    }
}

#[component]
pub(super) fn AdminRulesLoadError(detail: String, mut reload_generation: Signal<u64>) -> Element {
    rsx! {
        div {
            class: "admin-rules-error",
            role: "alert",
            p { class: "admin-panel__note", "{detail}" }
            button {
                r#type: "button",
                onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                {t!("admin-rules-retry")}
            }
        }
    }
}
