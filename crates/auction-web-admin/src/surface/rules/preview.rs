//! 规则 Markdown 预览组件。

use auction_web_shared::t;
use auction_web_shared::{MarkdownBlocks, without_leading_title};
use dioxus::prelude::*;

use crate::api::AdminRulesCandidate;

/// 展示已进入当前编辑状态的规则草稿，不执行发布或远程写入。
#[component]
pub(super) fn AdminRulesPreview(draft: AdminRulesCandidate) -> Element {
    let markdown = without_leading_title(&draft.markdown);
    let empty = markdown.trim().is_empty();
    let title = draft.title.trim();
    let label = if title.is_empty() { t!("admin-rules-untitled") } else { title.to_owned() };
    rsx! {
        div { class: "admin-rules-preview",
            h2 { {t!("admin-rules-preview")} }
            article { aria_label: label, class: "shared-markdown",
                MarkdownBlocks { markdown }
            }
            if empty {
                p { {t!("admin-rules-empty-preview")} }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use auction_web_shared::build_bundle;
    use dioxus::dioxus_core::{AttributeValue, Mutation};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::AdminRulesPreview;
    use crate::api::AdminRulesCandidate;
    use crate::surface::ADMIN_LOCALE_CATALOG;

    #[component]
    fn PreviewHarness(markdown: String) -> Element {
        let catalog = &ADMIN_LOCALE_CATALOG;
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &catalog.default_locale,
                catalog.default_ftl,
                &catalog.default_locale,
                None,
            )
        })
        .unwrap();
        rsx! {
            AdminRulesPreview {
                draft: AdminRulesCandidate {
                    locale: "en".to_owned(),
                    title: "Published document name".to_owned(),
                    markdown,
                },
            }
        }
    }

    #[test]
    fn document_title_names_the_article_and_only_public_body_is_visible() {
        let mut dom = VirtualDom::new_with_props(
            PreviewHarness,
            PreviewHarnessProps { markdown: "# Removed heading\n\nVisible body".to_owned() },
        );
        let edits = dom.rebuild_to_vec().edits;
        let text = |expected: &str| {
            edits.iter().any(
                |edit| matches!(edit, Mutation::CreateTextNode { value, .. } if value == expected),
            )
        };
        assert!(text("Visible body"));
        assert!(!text("Removed heading"));
        assert!(!text("Published document name"));
        assert!(edits.iter().any(|edit| matches!(
            edit,
            Mutation::SetAttribute { name: "aria-label", value: AttributeValue::Text(value), .. }
                if value == "Published document name"
        )));
    }
}
