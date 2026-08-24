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
    match level {
        1 => rsx! { h1 { MarkdownInline { nodes } } },
        2 => rsx! { h2 { MarkdownInline { nodes } } },
        3 => rsx! { h3 { MarkdownInline { nodes } } },
        4 => rsx! { h4 { MarkdownInline { nodes } } },
        5 => rsx! { h5 { MarkdownInline { nodes } } },
        _ => rsx! { h6 { MarkdownInline { nodes } } },
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
                Inline::Link { label, target } => rsx! {
                    a {
                        key: "{index}",
                        href: target,
                        target: "_blank",
                        rel: "noreferrer",
                        MarkdownInline { nodes: label }
                    }
                },
                Inline::Image { alt, source } => rsx! {
                    img { key: "{index}", src: source, alt, loading: "lazy" }
                },
            }
        }
    }
}
