//! Public 站点文档页面：规则与关于共用同一套读取、语言跟随与 Markdown 排版。

use dioxus::prelude::*;

use auction_web_shared::t;

use super::use_content_loading;
use crate::api::{DOCUMENT_UNPUBLISHED, RulesDocument, SiteDocument, fetch_site_document};
use auction_web_shared::{MarkdownBlocks, current_language, without_leading_title};

pub(super) fn page(kind: SiteDocument) -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    // `current_language()` 读取 context，是 hook；只能在组件渲染期调用，不能放进 resource 闭包。
    let locale = current_language().to_string();
    // `use_reactive` 把语言作为 resource 的显式依赖：语言切换时重新取回对应语言的正文，
    // 同时保留 refresh_generation 触发的重试。
    let document = use_resource(use_reactive((&locale,), move |(requested,)| {
        let _ = refresh_generation();
        async move {
            let result = fetch_site_document(kind, &requested).await;
            (requested, result)
        }
    }));
    let loaded = document.read_unchecked();
    // 重读保留的旧正文与错误只属于原语言，不能随新语言的页头一起展示。
    let current = loaded.as_ref().filter(|(requested, _)| requested == &locale);
    use_content_loading(current.is_none());
    // 页面名称只作为区域的可访问名称；加载失败或尚未发布时没有文档标题可用。
    let page_name = match kind {
        SiteDocument::Rules => t!("rules-title-fallback"),
        SiteDocument::About => t!("page-about-title"),
    };
    match current.map(|(_, result)| result) {
        None => rsx! {},
        // “关于”是可选内容：尚未发布是确定的空状态，不提供重试。
        Some(Err(code)) if code == DOCUMENT_UNPUBLISHED => rsx! {
            article { class: "public-rules-page", aria_label: page_name,
                div { class: "public-auction-empty", role: "status",
                    p { class: "public-auction-empty__title", {t!("about-empty-title")} }
                    p { class: "public-auction-empty__hint", {t!("about-empty-hint")} }
                }
            }
        },
        Some(Err(_)) => rsx! {
            article { class: "public-rules-page", aria_label: page_name,
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
            SiteDocumentView { document: document.clone() }
        },
    }
}

#[component]
fn SiteDocumentView(document: RulesDocument) -> Element {
    rsx! {
        // 文档标题与导航里的入口同名，只作为区域名称；正文从第一节开始。
        article { class: "public-rules-page", aria_label: "{document.title}",
            div { class: "public-rules-page__document",
                MarkdownBlocks { markdown: without_leading_title(&document.markdown) }
            }
        }
    }
}
