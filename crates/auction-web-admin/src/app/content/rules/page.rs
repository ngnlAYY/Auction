//! 规则页面加载、语言切换和 workspace 装配。

use auction_web_shared::legacy_t as t;
use auction_web_shared::{AsyncState, AsyncStateKind, SUPPORTED_LOCALES};
use dioxus::prelude::*;

use crate::api::fetch_admin_rules;
use crate::app::surface::admin_api_error_message;
use crate::app::surface::pending::{PendingCommandPanel, use_pending_commands};

use super::helpers::{DEFAULT_RULES_LOCALE, missing_rules_document, to_draft};
use super::model::RuleNotice;
use super::workspace::AdminRulesWorkspace;

/// 读取并展示 Admin 规则文档；语言切换会让请求带上旧版约定的 `lang` 查询参数。
pub fn page() -> Element {
    let target = use_signal(|| DEFAULT_RULES_LOCALE.to_owned());
    let locales = use_signal(|| {
        SUPPORTED_LOCALES.iter().map(|locale| locale.code.to_owned()).collect::<Vec<_>>()
    });
    let new_locale = use_signal(String::new);
    let reload_generation = use_signal(|| 0_u64);
    let pending = use_pending_commands(reload_generation);
    let mut draft = use_signal(|| None::<crate::api::AdminRulesCandidate>);
    let mut checked_signature = use_signal(String::new);
    let mut notice = use_signal(|| None::<RuleNotice>);
    let busy = use_signal(|| false);
    let rules = use_resource(move || {
        let _ = reload_generation();
        let locale = target();
        async move {
            let result = fetch_admin_rules(locale.clone()).await;
            (locale, result)
        }
    });

    // 规则请求成功后才初始化编辑器，避免用户编辑过程中普通重绘把草稿覆盖掉。
    use_effect(move || {
        let requested_locale = target();
        let state = rules.state().cloned();
        let snapshot = rules.read_unchecked();
        if state == UseResourceState::Ready
            && let Some((loaded_locale, Ok(document))) = snapshot.as_ref()
            && loaded_locale == &requested_locale
            && document.locale == requested_locale
        {
            draft.set(Some(to_draft(document, &requested_locale)));
            checked_signature.set(String::new());
            notice.set(None);
        }
    });

    // Resource 重读时保留旧值；旧语言的文档与错误均不能装配成新语言编辑器。
    let snapshot = rules.read_unchecked();
    let current = if rules.state().cloned() == UseResourceState::Ready {
        snapshot
            .as_ref()
            .filter(|(loaded_locale, _)| loaded_locale == &target())
            .map(|(_, result)| result)
    } else {
        None
    };
    let content = match current {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-rules-loading"),
                detail: "",
            }
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
            PendingCommandPanel { owner: pending }
            {content}
        }
    }
}

#[component]
pub(super) fn AdminRulesLoadError(detail: String, mut reload_generation: Signal<u64>) -> Element {
    rsx! {
        AsyncState {
            kind: AsyncStateKind::Error,
            title: detail,
            detail: "",
            retry_label: t!("admin-rules-retry"),
            on_retry: move |_| reload_generation.set(reload_generation().saturating_add(1)),
        }
    }
}
