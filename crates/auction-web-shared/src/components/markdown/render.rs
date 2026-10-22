//! Markdown 结构到安全 DOM 的渲染。

use dioxus::prelude::*;

use super::inline::is_internal_target;
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
        MarkdownBlock::Table { headers, rows } => {
            let widths = table_column_weights(&headers, &rows);
            rsx! {
            div { class: "shared-markdown__table-wrap",
                table {
                    crate::TableColumns { layout: "markdown-content", widths }
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
            }
        }
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

/// 文档表格没有固定业务字段：按表头与前 32 行的典型可见长度（第 75 百分位、CJK 双宽）
/// 估算列权重，避免单条长 URL/说明挤占整张表。
fn table_column_weights(headers: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>]) -> Vec<u16> {
    headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            let mut lengths = rows
                .iter()
                .take(32)
                .filter_map(|row| row.get(index))
                .map(|nodes| visible_units(nodes))
                .collect::<Vec<_>>();
            lengths.sort_unstable();
            let typical = lengths.get(lengths.len() * 3 / 4).copied().unwrap_or(0);
            visible_units(header).max(typical).clamp(8, 48)
        })
        .collect()
}

fn visible_units(nodes: &[Inline]) -> u16 {
    count_visible_units(nodes, &mut 96)
}

/// 列宽只采样前 96 个可见字符；直接遍历节点，避免先复制完整单元格再丢弃长后缀。
fn count_visible_units(nodes: &[Inline], remaining: &mut usize) -> u16 {
    let mut units = 0;
    for node in nodes {
        if *remaining == 0 {
            break;
        }
        match node {
            Inline::Text(value) | Inline::Code(value) | Inline::Image { alt: value, .. } => {
                for character in value.chars().take(*remaining) {
                    units += if character.is_ascii() { 1 } else { 2 };
                    *remaining -= 1;
                }
            }
            Inline::Strong(nodes) | Inline::Emphasis(nodes) | Inline::Strike(nodes) => {
                units += count_visible_units(nodes, remaining);
            }
            Inline::Link { label, .. } => units += count_visible_units(label, remaining),
        }
    }
    units
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

/// 从标题行内节点提取纯文本并 slug 化为锚点 id；仅保留字母数字（含 CJK），其余折叠为
/// 单个连字符。文本为空或全是标点时返回 `None`，不生成空 id。
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

/// 递归收集标题行内节点的可见纯文本，供锚点生成使用。
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
