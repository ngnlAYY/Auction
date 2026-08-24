//! 规则 Markdown 预览组件。

use auction_web_shared::{MarkdownDocument, without_leading_title};
use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::AdminRulesCandidate;

/// 展示已进入当前编辑状态的规则草稿，不执行发布或远程写入。
#[component]
pub(super) fn AdminRulesPreview(draft: AdminRulesCandidate) -> Element {
    let markdown = without_leading_title(&draft.markdown);
    rsx! {
        article {
            class: "admin-rules__preview",
            aria_labelledby: "admin-rules-preview-title",
            h2 { id: "admin-rules-preview-title", {t!("admin-rules-preview")} }
            h1 {
                if draft.title.trim().is_empty() {
                    {t!("admin-rules-untitled")}
                } else {
                    "{draft.title}"
                }
            }
            if markdown.trim().is_empty() {
                p { class: "admin-rules__preview-empty", {t!("admin-rules-empty-preview")} }
            } else {
                MarkdownDocument { markdown }
            }
        }
    }
}
