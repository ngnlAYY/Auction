//! 独立于 runtime 文件配置能力的只读开关卡片。

use crate::api::fetch_runtime_switches;
use auction_web_shared::{AsyncState, AsyncStateKind, legacy_t as t};
use dioxus::prelude::*;

#[component]
pub(super) fn RuntimeSwitchCard() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let snapshot = use_resource(move || {
        let _ = refresh();
        fetch_runtime_switches()
    });
    let current = snapshot.read_unchecked().clone();
    rsx! {
        section { class: "admin-runtime-switches admin-panel",
            h2 { {t!("admin-switches-title")} }
            p { class: "admin-panel__note", {t!("admin-switches-note")} }
            button { class: "admin-btn-secondary", r#type: "button", onclick: move |_| refresh.set(refresh().saturating_add(1)), {t!("admin-switches-refresh")} }
            match current {
                None => rsx! { AsyncState { kind: AsyncStateKind::Loading, title: t!("admin-switches-loading"), detail: "" } },
                Some(Err(code)) => rsx! { AsyncState { kind: AsyncStateKind::Error, title: super::super::admin_api_error_message(&code), detail: "" } },
                Some(Ok(data)) => rsx! {
                    p { {t!("admin-switches-age")} ": " {data.admin_last_success_age_seconds.map(|age| format!("{age:.1} s")).unwrap_or_else(|| t!("admin-switches-unconfirmed"))} }
                    if data.admin_last_success_age_seconds.is_none_or(|age| age > 10.0) {
                        p { class: "admin-panel__note", {t!("admin-switches-stale")} }
                    }
                    for intent in data.database_intent {
                        article { class: "admin-runtime-switches__entry",
                            h3 { "{intent.name}" }
                            p { {t!("admin-switches-intent")} ": " {intent.operation.clone().unwrap_or_else(|| t!("admin-switches-unset"))} " / " {intent.value.map(|value| value.to_string()).unwrap_or_else(|| "—".to_owned())} " / r{intent.revision}" }
                            p { {t!("admin-switches-expiry")} ": " {intent.expires_at.map(|until| until.to_rfc3339()).unwrap_or_else(|| "—".to_owned())} }
                            p { {t!("admin-switches-actor")} ": " {intent.actor_id.unwrap_or_else(|| "—".to_owned())} " / " {intent.entrypoint.unwrap_or_else(|| "—".to_owned())} }
                            p { {t!("admin-switches-reason")} ": " {intent.reason.unwrap_or_else(|| "—".to_owned())} }
                            for applied in data.admin_applied.iter().filter(|value| value.name == intent.name) {
                                p { {t!("admin-switches-applied")} ": " {applied.effective_value.to_string()} " / r{applied.revision} / " {t!("admin-switches-baseline")} ": " {applied.toml_baseline.to_string()} }
                            }
                        }
                    }
                },
            }
        }
    }
}
