//! 规则 Markdown 预览组件。

use auction_web_shared::t;
use auction_web_shared::{MarkdownDocument, without_leading_title};
use dioxus::prelude::*;

use crate::api::AdminRulesCandidate;

/// 展示已进入当前编辑状态的规则草稿，不执行发布或远程写入。
#[component]
pub(super) fn AdminRulesPreview(draft: AdminRulesCandidate) -> Element {
    let markdown = without_leading_title(&draft.markdown);
    rsx! {
        article {
            aria_labelledby: "rules-preview-title",
            class: "admin-rules-preview",
            h2 { id: "rules-preview-title", {t!("admin-rules-preview")} }
            h1 {
                if draft.title.is_empty() {
                    {t!("admin-rules-untitled")}
                } else {
                    "{draft.title}"
                }
            }
            if markdown.is_empty() {
                p { {t!("admin-rules-empty-preview")} }
            } else {
                MarkdownDocument { markdown }
            }
        }
    }
}
