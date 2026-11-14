//! 站点文档（规则、关于）页面加载、语言切换和 workspace 装配。

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind};
use dioxus::prelude::*;

use crate::api::{AdminDocumentKind, fetch_admin_rules};
use crate::surface::admin_api_error_message;
use crate::surface::pending::PendingCommandPanel;

use super::helpers::{missing_rules_document, to_draft};
use super::model::{DocumentDrafts, RuleNotice, RulesBaseline, RulesDraft};
use super::workspace::AdminRulesWorkspace;

/// 读取并展示一种 Admin 站点文档；语言切换会让请求带上旧版约定的 `lang` 查询参数。
pub fn page(kind: AdminDocumentKind) -> Element {
    let RulesDraft {
        target,
        locales,
        mut draft,
        mut baseline,
        mut checked_candidate,
        mut notice,
        busy,
        reload_generation,
        pending,
        dirty,
    } = use_context::<DocumentDrafts>().get(kind);
    let new_locale = use_signal(String::new);
    let rules = use_resource(move || {
        let generation = reload_generation();
        let locale = target();
        async move {
            let result = fetch_admin_rules(kind, locale.clone()).await;
            ((locale, generation), result)
        }
    });

    // 回页时先核对权威语言与版本；草稿有修改时绝不被读取结果覆盖。
    use_effect(move || {
        let requested_locale = target();
        let requested_generation = reload_generation();
        if rules.state().cloned() != UseResourceState::Ready {
            return;
        }
        let snapshot = rules.read_unchecked();
        let Some(((loaded_locale, loaded_generation), result)) = snapshot.as_ref() else { return };
        if loaded_locale != &requested_locale || *loaded_generation != requested_generation {
            return;
        }
        let missing;
        let document = match result {
            Ok(document) if document.locale == requested_locale => document,
            Err(error) if error.status == Some(404) => {
                missing = missing_rules_document(requested_locale.clone());
                &missing
            }
            _ => return,
        };
        let has_edits = baseline.peek().as_ref().is_some_and(|base| {
            base.matches_locale(&requested_locale)
                && draft.peek().as_ref().is_some_and(|candidate| !base.matches_candidate(candidate))
        });
        if has_edits {
            if baseline.peek().as_ref().is_none_or(|base| !base.matches_document(document)) {
                checked_candidate.set(None);
                notice.set(None);
            }
            return;
        }
        baseline.set(Some(RulesBaseline::from_document(document)));
        draft.set(Some(to_draft(document, &requested_locale)));
        checked_candidate.set(None);
        // 成功反馈属于已完成的发布，不因后续权威快照落地而消失。
        if !matches!(notice.peek().as_ref(), Some(RuleNotice::Published)) {
            notice.set(None);
        }
    });

    // Resource 重读时保留旧值；旧语言或旧世代的响应均不能装配编辑器。
    let snapshot = rules.read_unchecked();
    let current = if rules.state().cloned() == UseResourceState::Ready {
        snapshot
            .as_ref()
            .filter(|((loaded_locale, generation), _)| {
                loaded_locale == &target() && *generation == reload_generation()
            })
            .map(|(_, result)| result)
    } else {
        None
    };
    let baseline_ready =
        baseline.read().as_ref().is_some_and(|base| base.matches_locale(&target()));
    let needs_baseline = current.is_some_and(|result| match result {
        Ok(_) => true,
        Err(error) => error.status == Some(404),
    });
    let current = if needs_baseline && !baseline_ready { None } else { current };
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
                    kind,
                    document,
                    target: target(),
                    locales,
                    new_locale,
                    target_signal: target,
                    draft,
                    checked_candidate,
                    baseline,
                    notice,
                    busy,
                    reload_generation,
                    pending,
                    dirty,
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
                kind,
                document: document.clone(),
                target: target(),
                locales,
                new_locale,
                target_signal: target,
                draft,
                checked_candidate,
                baseline,
                notice,
                busy,
                reload_generation,
                pending,
                dirty,
            }
        },
    };

    rsx! {
        section {
            class: "admin-rules-page",
            aria_labelledby: "rules-title",
            h1 { id: "rules-title",
                match kind {
                    AdminDocumentKind::Rules => t!("page-rules-title"),
                    AdminDocumentKind::About => t!("page-about-title"),
                }
            }
            p { class: "admin-rules-intro",
                match kind {
                    AdminDocumentKind::Rules => t!("admin-rules-description"),
                    AdminDocumentKind::About => t!("admin-about-description"),
                }
            }
            PendingCommandPanel { owner: pending, session_scoped: true }
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
