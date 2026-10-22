//! 共享 Markdown 文档组件及其解析边界。

mod blocks;
mod inline;
mod model;
mod render;

use dioxus::prelude::*;
use render::MarkdownBlockView;

/// 将规则正文渲染为不执行原始 HTML 的 DOM。
/// 只支持旧版规则页实际使用的 Markdown/GFM 子集；文本节点由 Dioxus 自动转义，链接只允许
/// HTTP(S)、站内相对路径、锚点和 mailto 目标，拒绝反斜杠与控制字符。
/// 单个链接标签/目标限 1/4 KiB，超限按文本处理；正文不因链接窗口限制而截断。
#[component]
pub fn MarkdownDocument(markdown: String) -> Element {
    rsx! {
        article { class: "shared-markdown",
            MarkdownBlocks { markdown }
        }
    }
}

/// 只渲染 Markdown 的块级节点，不添加外层容器；供页面自带文档容器的场景使用。
#[component]
pub fn MarkdownBlocks(markdown: String) -> Element {
    let blocks = blocks::parse_blocks(&markdown);
    rsx! {
        for (index, block) in blocks.into_iter().enumerate() {
            MarkdownBlockView { key: "{index}", block }
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
