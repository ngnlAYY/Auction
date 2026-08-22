//! Public 规则文档页面。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{RulesDocument, fetch_rules};

use super::Placeholder;

pub(super) fn page() -> Element {
    let rules = use_resource(fetch_rules);
    match &*rules.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-rules-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-rules-title"), detail: t!("data-error") }
        },
        Some(Ok(document)) => rsx! {
            RulesDocumentView { document: document.clone() }
        },
    }
}

#[component]
fn RulesDocumentView(document: RulesDocument) -> Element {
    rsx! {
        section { class: "public-auction-category", aria_labelledby: "rules-title",
            h1 { id: "rules-title", class: "public-auction-category__title", "{document.title}" }
            p { class: "public-auction-page__status",
                {t!("rules-revision", revision: document.revision, locale: document.locale)}
            }
            p { class: "public-auction-page__status",
                {t!("rules-published", published_at: document.published_at)}
            }
            pre { class: "public-auction-page__document", "{document.markdown}" }
        }
    }
}
