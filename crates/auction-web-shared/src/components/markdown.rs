use dioxus::prelude::*;

/// 将规则正文渲染为不执行原始 HTML 的 DOM。
///
/// 规则正文来自管理端保存的内容，但浏览器端仍不应把 HTML 或不安全链接直接交给 DOM。
/// 这里实现旧版规则页实际使用的常见 Markdown/GFM 展示子集；所有文本节点由 Dioxus
/// 自动转义，链接只允许 HTTP(S)、站内相对路径、锚点和 mailto 目标。
#[component]
pub fn MarkdownDocument(markdown: String) -> Element {
    let blocks = parse_blocks(&markdown);
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum MarkdownBlock {
    Heading { level: u8, nodes: Vec<Inline> },
    Paragraph(Vec<Inline>),
    UnorderedList(Vec<Vec<Inline>>),
    OrderedList { start: usize, items: Vec<Vec<Inline>> },
    Quote(Vec<Inline>),
    Table { headers: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>> },
    Code { language: Option<String>, value: String },
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Inline {
    Text(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Strike(Vec<Inline>),
    Code(String),
    Link { label: Vec<Inline>, target: String },
    Image { alt: String, source: String },
}

#[component]
fn MarkdownBlockView(block: MarkdownBlock) -> Element {
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

fn parse_blocks(markdown: &str) -> Vec<MarkdownBlock> {
    let markdown = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    let lines = markdown.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let trimmed = lines[index].trim();
        if trimmed.is_empty() {
            index += 1;
            continue;
        }

        if let Some((fence, language)) = fence_start(trimmed) {
            let mut value = Vec::new();
            index += 1;
            while index < lines.len() {
                let candidate = lines[index].trim_start();
                if candidate.starts_with(fence) {
                    index += 1;
                    break;
                }
                value.push(lines[index]);
                index += 1;
            }
            blocks.push(MarkdownBlock::Code { language, value: value.join("\n") });
            continue;
        }

        if let Some((level, value)) = heading(trimmed) {
            blocks.push(MarkdownBlock::Heading { level, nodes: parse_inline(value) });
            index += 1;
            continue;
        }

        if is_rule(trimmed) {
            blocks.push(MarkdownBlock::Rule);
            index += 1;
            continue;
        }

        if index + 1 < lines.len()
            && trimmed.contains('|')
            && is_table_separator(lines[index + 1].trim())
        {
            let headers =
                split_table_row(trimmed).into_iter().map(|value| parse_inline(&value)).collect();
            index += 2;
            let mut rows = Vec::new();
            while index < lines.len() {
                let row = lines[index].trim();
                if row.is_empty() || !row.contains('|') {
                    break;
                }
                rows.push(
                    split_table_row(row).into_iter().map(|value| parse_inline(&value)).collect(),
                );
                index += 1;
            }
            blocks.push(MarkdownBlock::Table { headers, rows });
            continue;
        }

        if trimmed.starts_with('>') {
            let mut quote = Vec::new();
            while index < lines.len() {
                let candidate = lines[index].trim_start();
                let Some(value) = candidate.strip_prefix('>') else {
                    break;
                };
                quote.push(value.strip_prefix(' ').unwrap_or(value).trim());
                index += 1;
            }
            blocks.push(MarkdownBlock::Quote(parse_inline(&quote.join("\n"))));
            continue;
        }

        if unordered_item(trimmed).is_some() {
            let mut items = Vec::new();
            while index < lines.len() {
                let candidate = lines[index].trim();
                let Some(value) = unordered_item(candidate) else {
                    break;
                };
                items.push(parse_inline(value));
                index += 1;
            }
            blocks.push(MarkdownBlock::UnorderedList(items));
            continue;
        }

        if let Some((start, value)) = ordered_item(trimmed) {
            let mut items = vec![parse_inline(value)];
            index += 1;
            while index < lines.len() {
                let candidate = lines[index].trim();
                let Some((_, value)) = ordered_item(candidate) else {
                    break;
                };
                items.push(parse_inline(value));
                index += 1;
            }
            blocks.push(MarkdownBlock::OrderedList { start, items });
            continue;
        }

        let mut paragraph = vec![lines[index].trim()];
        index += 1;
        while index < lines.len() {
            let candidate = lines[index].trim();
            if candidate.is_empty() || starts_block(&lines, index) {
                break;
            }
            paragraph.push(candidate);
            index += 1;
        }
        blocks.push(MarkdownBlock::Paragraph(parse_inline(&paragraph.join("\n"))));
    }

    blocks
}

fn starts_block(lines: &[&str], index: usize) -> bool {
    let trimmed = lines[index].trim();
    fence_start(trimmed).is_some()
        || heading(trimmed).is_some()
        || is_rule(trimmed)
        || trimmed.starts_with('>')
        || unordered_item(trimmed).is_some()
        || ordered_item(trimmed).is_some()
        || (index + 1 < lines.len()
            && trimmed.contains('|')
            && is_table_separator(lines[index + 1].trim()))
}

fn fence_start(value: &str) -> Option<(char, Option<String>)> {
    let (fence, rest) = if let Some(rest) = value.strip_prefix("```") {
        ('`', rest)
    } else {
        let rest = value.strip_prefix("~~~")?;
        ('~', rest)
    };
    let language = rest.trim();
    Some((fence, (!language.is_empty()).then(|| language.to_owned())))
}

fn heading(value: &str) -> Option<(u8, &str)> {
    let level = value.chars().take_while(|character| *character == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let content = &value[level..];
    let content = content.strip_prefix(' ').or_else(|| content.strip_prefix('\t'))?;
    Some((level as u8, content.trim()))
}

fn is_rule(value: &str) -> bool {
    let compact = value.replace(' ', "");
    compact.len() >= 3
        && (compact.chars().all(|character| character == '-')
            || compact.chars().all(|character| character == '*')
            || compact.chars().all(|character| character == '_'))
}

fn is_table_separator(value: &str) -> bool {
    let cells = split_table_row(value);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            cell.len() >= 3 && cell.chars().all(|character| character == '-' || character == ':')
        })
}

fn split_table_row(value: &str) -> Vec<String> {
    let value = value.trim();
    let value = value.strip_prefix('|').unwrap_or(value);
    let value = value.strip_suffix('|').unwrap_or(value);
    value.split('|').map(|cell| cell.trim().to_owned()).collect()
}

fn unordered_item(value: &str) -> Option<&str> {
    value
        .strip_prefix("- ")
        .or_else(|| value.strip_prefix("* "))
        .or_else(|| value.strip_prefix("+ "))
}

fn ordered_item(value: &str) -> Option<(usize, &str)> {
    let separator = value.find(". ")?;
    let start = value[..separator].parse().ok()?;
    Some((start, &value[separator + 2..]))
}

fn parse_inline(value: &str) -> Vec<Inline> {
    let mut nodes = Vec::new();
    let mut text = String::new();
    let mut index = 0;

    while index < value.len() {
        let rest = &value[index..];
        if rest.starts_with('\\')
            && let Some(character) = rest[1..].chars().next()
        {
            text.push(character);
            index += 1 + character.len_utf8();
            continue;
        }

        if let Some((label, target, consumed)) = bracket_link(rest, true)
            && let Some(source) = safe_link_target(&target)
        {
            flush_text(&mut nodes, &mut text);
            nodes.push(Inline::Image { alt: label, source });
            index += consumed;
            continue;
        }

        if let Some((label, target, consumed)) = bracket_link(rest, false)
            && let Some(target) = safe_link_target(&target)
        {
            flush_text(&mut nodes, &mut text);
            nodes.push(Inline::Link { label: parse_inline(&label), target });
            index += consumed;
            continue;
        }

        let mut handled = false;
        for (marker, kind) in [
            ("**", InlineKind::Strong),
            ("__", InlineKind::Strong),
            ("~~", InlineKind::Strike),
            ("*", InlineKind::Emphasis),
            ("_", InlineKind::Emphasis),
        ] {
            if let Some(inner) = rest.strip_prefix(marker)
                && let Some(close) = inner.find(marker)
                && close > 0
            {
                flush_text(&mut nodes, &mut text);
                let nested = parse_inline(&inner[..close]);
                nodes.push(kind.build(nested));
                index += marker.len() + close + marker.len();
                handled = true;
                break;
            }
        }
        if handled {
            continue;
        }

        if let Some(inner) = rest.strip_prefix('`')
            && let Some(close) = inner.find('`')
        {
            flush_text(&mut nodes, &mut text);
            nodes.push(Inline::Code(inner[..close].trim().to_owned()));
            index += close + 2;
            continue;
        }

        if (rest.starts_with("<http://") || rest.starts_with("<https://"))
            && let Some(close) = rest.find('>')
            && let Some(target) = safe_link_target(&rest[1..close])
        {
            flush_text(&mut nodes, &mut text);
            nodes.push(Inline::Link {
                label: vec![Inline::Text(rest[1..close].to_owned())],
                target,
            });
            index += close + 1;
            continue;
        }

        let character = rest.chars().next().unwrap_or_default();
        text.push(if character == '\n' { ' ' } else { character });
        index += character.len_utf8();
    }
    flush_text(&mut nodes, &mut text);
    nodes
}

#[derive(Clone, Copy)]
enum InlineKind {
    Strong,
    Emphasis,
    Strike,
}

impl InlineKind {
    fn build(self, nodes: Vec<Inline>) -> Inline {
        match self {
            Self::Strong => Inline::Strong(nodes),
            Self::Emphasis => Inline::Emphasis(nodes),
            Self::Strike => Inline::Strike(nodes),
        }
    }
}

fn bracket_link(value: &str, image: bool) -> Option<(String, String, usize)> {
    let prefix_length = if image { 2 } else { 1 };
    if image {
        if !value.starts_with("![") {
            return None;
        }
    } else if !value.starts_with('[') {
        return None;
    }
    let close_label = value[prefix_length..].find("](")? + prefix_length;
    let close_target = value[close_label + 2..].find(')')? + close_label + 2;
    let label = value[prefix_length..close_label].to_owned();
    let target = value[close_label + 2..close_target].trim().to_owned();
    Some((label, target, close_target + 1))
}

fn safe_link_target(value: &str) -> Option<String> {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    let allowed = lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || value.starts_with('#')
        || value.starts_with('?');
    allowed.then(|| value.to_owned())
}

fn flush_text(nodes: &mut Vec<Inline>, text: &mut String) {
    if !text.is_empty() {
        nodes.push(Inline::Text(std::mem::take(text)));
    }
}
