//! Public 规则文档页面。

use dioxus::prelude::*;

use auction_web_shared::legacy_t as t;

use super::use_content_loading;
use crate::api::{RulesDocument, fetch_rules};
use auction_web_shared::{MarkdownBlocks, current_language, without_leading_title};

pub(super) fn page() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    // `current_language()` 读取 context，是 hook；只能在组件渲染期调用，不能放进 resource 闭包。
    let locale = current_language().to_string();
    // `use_reactive` 把语言作为 resource 的显式依赖：语言切换时重新取回对应 FTL 后的规则正文，
    // 同时保留 refresh_generation 触发的重试。
    let rules = use_resource(use_reactive((&locale,), move |(requested,)| {
        let _ = refresh_generation();
        async move {
            let result = fetch_rules(&requested).await;
            (requested, result)
        }
    }));
    let loaded = rules.read_unchecked();
    // 重读保留的旧正文与错误只属于原语言，不能随新语言的页头一起展示。
    let current = loaded.as_ref().filter(|(requested, _)| requested == &locale);
    use_content_loading(current.is_none());
    match current.map(|(_, result)| result) {
        None => rsx! {},
        Some(Err(_)) => rsx! {
            article { class: "public-rules-page", aria_label: t!("rules-title-fallback"),
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
        // 文档标题与导航里的规则入口同名，只作为区域名称；正文从第一节开始。
        article { class: "public-rules-page", aria_label: "{document.title}",
            div { class: "public-rules-page__document",
                MarkdownBlocks { markdown: without_leading_title(&document.markdown) }
            }
        }
    }
}
