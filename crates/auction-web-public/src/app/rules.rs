//! Public 规则文档页面。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{RulesDocument, fetch_rules};
use auction_web_shared::{MarkdownDocument, without_leading_title};

use super::{Placeholder, RetryPlaceholder, format_auction_time};

pub(super) fn page() -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let rules = use_resource(move || {
        let _ = refresh_generation();
        fetch_rules()
    });
    match &*rules.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-rules-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("page-rules-title"),
                detail: t!("data-error"),
                refresh_generation,
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
        section { class: "public-auction-category", aria_labelledby: "rules-title",
            h1 { id: "rules-title", class: "public-auction-category__title", "{document.title}" }
            p { class: "public-auction-page__status",
                {t!("rules-revision", revision: document.revision, locale: document.locale)}
            }
            p { class: "public-auction-page__status",
                {t!("rules-published", published_at: format_auction_time(&document.published_at))}
            }
            MarkdownDocument { markdown: without_leading_title(&document.markdown) }
        }
    }
}
