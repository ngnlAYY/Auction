//! Markdown 结构到安全 DOM 的渲染。

use dioxus::prelude::*;

use super::model::{Inline, MarkdownBlock};

#[component]
pub(super) fn MarkdownBlockView(block: MarkdownBlock) -> Element {
    match block {
        MarkdownBlock::Heading { level, nodes } => render_heading(level, nodes),
        MarkdownBlock::Paragraph(nodes) => rsx! {
            p { MarkdownInline { nodes } }
        },
        MarkdownBlock::UnorderedList(items) => rsx! {
            ul {
                for (index, nodes) in items.into_iter().enumerate() {
                    li { key: "{index}", MarkdownInline { nodes } }
                }
            }
        },
        MarkdownBlock::OrderedList { start, items } => rsx! {
            ol {
                start: start.to_string(),
                for (index, nodes) in items.into_iter().enumerate() {
                    li { key: "{index}", MarkdownInline { nodes } }
                }
            }
        },
        MarkdownBlock::Quote(nodes) => rsx! {
            blockquote { MarkdownInline { nodes } }
        },
        MarkdownBlock::Table { headers, rows } => rsx! {
            div { class: "shared-markdown__table-wrap",
                table {
                    thead {
                        tr {
                            for (index, nodes) in headers.into_iter().enumerate() {
                                th { key: "header-{index}", MarkdownInline { nodes } }
                            }
                        }
                    }
                    tbody {
                        for (row_index, row) in rows.into_iter().enumerate() {
                            tr {
                                key: "row-{row_index}",
                                for (cell_index, nodes) in row.into_iter().enumerate() {
                                    td { key: "cell-{row_index}-{cell_index}", MarkdownInline { nodes } }
                                }
                            }
                        }
                    }
                }
            }
        },
        MarkdownBlock::Code { language, value } => rsx! {
            pre {
                if let Some(language) = language {
                    code { class: "language-{language}", "{value}" }
                } else {
                    code { "{value}" }
                }
            }
        },
        MarkdownBlock::Rule => rsx! { hr {} },
    }
}

fn render_heading(level: u8, nodes: Vec<Inline>) -> Element {
    // 标题生成 id 锚点，供目录或站内 # 链接跳转；无效 slug（如纯标点标题）则不生成 id。
    let anchor = heading_anchor(&nodes);
    match level {
        1 => rsx! { h1 { id: anchor, MarkdownInline { nodes } } },
        2 => rsx! { h2 { id: anchor, MarkdownInline { nodes } } },
        3 => rsx! { h3 { id: anchor, MarkdownInline { nodes } } },
        4 => rsx! { h4 { id: anchor, MarkdownInline { nodes } } },
        5 => rsx! { h5 { id: anchor, MarkdownInline { nodes } } },
        _ => rsx! { h6 { id: anchor, MarkdownInline { nodes } } },
    }
}

/// 从标题行内节点提取纯文本并 slug 化为锚点 id。
///
/// 仅保留字母数字字符（含 CJK 表意文字），其余字符折叠为单个连字符并去除首尾连字符；
/// 标题文本为空或全是标点时返回 `None`，不生成空 id。
fn heading_anchor(nodes: &[Inline]) -> Option<String> {
    let mut text = String::new();
    collect_text(nodes, &mut text);

    let mut slug = String::new();
    for character in text.trim().chars() {
        if character.is_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    (!slug.is_empty()).then_some(slug)
}

/// 递归收集行内节点的可见纯文本，供标题锚点 slug 化使用。
fn collect_text(nodes: &[Inline], output: &mut String) {
    for node in nodes {
        match node {
            Inline::Text(value) | Inline::Code(value) => output.push_str(value),
            Inline::Strong(nodes) | Inline::Emphasis(nodes) | Inline::Strike(nodes) => {
                collect_text(nodes, output);
            }
            Inline::Link { label, .. } => collect_text(label, output),
            Inline::Image { alt, .. } => output.push_str(alt),
        }
    }
}

/// 站内链接（`/`、`./`、`../`、`#`、`?` 开头）留在当前页跳转；其余视为外链。
///
/// 与 inline.rs 的 `safe_link_target` 白名单保持同一口径：`//` 开头的协议相对 URL
/// 会跳到外部域，按外链处理。
fn is_internal_target(target: &str) -> bool {
    (target.starts_with('/') && !target[1..].starts_with('/'))
        || target.starts_with("./")
        || target.starts_with("../")
        || target.starts_with('#')
        || target.starts_with('?')
}

#[component]
fn MarkdownInline(nodes: Vec<Inline>) -> Element {
    rsx! {
        for (index, node) in nodes.into_iter().enumerate() {
            match node {
                Inline::Text(value) => rsx! { span { key: "{index}", "{value}" } },
                Inline::Strong(nodes) => rsx! { strong { key: "{index}", MarkdownInline { nodes } } },
                Inline::Emphasis(nodes) => rsx! { em { key: "{index}", MarkdownInline { nodes } } },
                Inline::Strike(nodes) => rsx! { del { key: "{index}", MarkdownInline { nodes } } },
                Inline::Code(value) => rsx! { code { key: "{index}", "{value}" } },
                Inline::Link { label, target } => {
                    // 站内链接在当前页跳转，避免整页刷新打断 SPA；外链才开新页并加 noreferrer。
                    if is_internal_target(&target) {
                        rsx! {
                            a {
                                key: "{index}",
                                href: target,
                                MarkdownInline { nodes: label }
                            }
                        }
                    } else {
                        rsx! {
                            a {
                                key: "{index}",
                                href: target,
                                target: "_blank",
                                rel: "noreferrer",
                                MarkdownInline { nodes: label }
                            }
                        }
                    }
                }
                Inline::Image { alt, source } => rsx! {
                    img { key: "{index}", src: source, alt, loading: "lazy" }
                },
            }
        }
    }
}
