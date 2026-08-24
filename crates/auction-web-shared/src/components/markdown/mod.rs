//! 共享 Markdown 文档组件及其解析边界。

mod blocks;
mod inline;
mod model;
mod render;

use dioxus::prelude::*;
use render::MarkdownBlockView;

/// 将规则正文渲染为不执行原始 HTML 的 DOM。
///
/// 规则正文来自管理端保存的内容，但浏览器端仍不应把 HTML 或不安全链接直接交给 DOM。
/// 这里实现旧版规则页实际使用的常见 Markdown/GFM 展示子集；所有文本节点由 Dioxus
/// 自动转义，链接只允许 HTTP(S)、站内相对路径、锚点和 mailto 目标。
#[component]
pub fn MarkdownDocument(markdown: String) -> Element {
    let blocks = blocks::parse_blocks(&markdown);
    rsx! {
        article { class: "shared-markdown",
            for (index, block) in blocks.into_iter().enumerate() {
                MarkdownBlockView { key: "{index}", block }
            }
        }
    }
}

/// 移除正文首行的一级标题，避免页面标题与 Markdown 标题重复显示。
pub fn without_leading_title(markdown: &str) -> String {
    let markdown = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    let mut lines = markdown.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    let is_title = first
        .strip_prefix('#')
        .is_some_and(|value| value.starts_with(' ') || value.starts_with('\t'));
    if !is_title {
        return markdown.to_owned();
    }
    let rest = lines.collect::<Vec<_>>().join("\n");
    rest.strip_prefix('\n').unwrap_or(&rest).to_owned()
}
