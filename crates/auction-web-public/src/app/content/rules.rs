//! Public 规则文档页面。

use dioxus::prelude::*;
use dioxus_i18n::prelude::*;

use auction_web_shared::legacy_t as t;

use crate::api::{RulesDocument, fetch_rules};
use auction_web_shared::{MarkdownBlocks, without_leading_title};

pub(super) fn page() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let i18n = i18n();
    let rules = use_resource(move || {
        let _ = refresh_generation();
        let locale = i18n.language().to_string();
        async move { fetch_rules(&locale).await }
    });
    match &*rules.read_unchecked() {
        None => rsx! {
            article { class: "public-rules-page", aria_labelledby: "rules-title",
                h1 { id: "rules-title", class: "public-visually-hidden",
                    {t!("rules-title-fallback")}
                }
                p { class: "public-page-status", role: "status", {t!("rules-loading")} }
            }
        },
        Some(Err(_)) => rsx! {
            article { class: "public-rules-page", aria_labelledby: "rules-title",
                h1 { id: "rules-title", class: "public-visually-hidden",
                    {t!("rules-title-fallback")}
                }
                div { role: "alert",
                    h1 { {t!("rules-load-error-title")} }
                    p { class: "public-page-status", {t!("rules-load-error-detail")} }
                    button {
                        class: "public-state-retry",
                        r#type: "button",
                        onclick: move |_| {
                            refresh_generation.set(refresh_generation().saturating_add(1));
                        },
                        {t!("data-retry")}
                    }
                }
            }
        },
        Some(Ok(document)) => rsx! {
            RulesDocumentView { document: document.clone() }
        },
    }
}

#[component]
fn RulesDocumentView(document: RulesDocument) -> Element {
    rsx! {
        article { class: "public-rules-page", aria_labelledby: "rules-title",
            h1 { id: "rules-title", "{document.title}" }
            div { class: "public-rules-page__document",
                MarkdownBlocks { markdown: without_leading_title(&document.markdown) }
            }
        }
    }
}
