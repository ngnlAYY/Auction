//! Markdown 块级语法解析。

use super::inline::parse_inline;
use super::model::MarkdownBlock;

pub(super) fn parse_blocks(markdown: &str) -> Vec<MarkdownBlock> {
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
                // 结束围栏必须与开头同类且至少同长；短标记或带正文的行仍属于代码。
                if candidate.strip_prefix(fence).is_some_and(|rest| {
                    rest.trim_start_matches(char::from(fence.as_bytes()[0])).trim().is_empty()
                }) {
                    index += 1;
                    break;
                }
                value.push(lines[index]);
                index += 1;
            }
            blocks.push(MarkdownBlock::Code {
                language: language.map(str::to_owned),
                value: value.join("\n"),
            });
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
            let headers = split_table_row(trimmed).map(parse_inline).collect();
            index += 2;
            let mut rows = Vec::new();
            while index < lines.len() {
                let row = lines[index].trim();
                if row.is_empty() || !row.contains('|') {
                    break;
                }
                rows.push(split_table_row(row).map(parse_inline).collect());
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

fn fence_start(value: &str) -> Option<(&str, Option<&str>)> {
    let marker = *value.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let length = value.bytes().take_while(|byte| *byte == marker).count();
    if length < 3 {
        return None;
    }
    let language = value[length..].trim();
    Some((&value[..length], (!language.is_empty()).then_some(language)))
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
    // 沿用只忽略 ASCII 空格的语法，不为每条候选行创建去空格副本。
    let mut bytes = value.bytes().filter(|byte| *byte != b' ');
    let Some(marker @ (b'-' | b'*' | b'_')) = bytes.next() else {
        return false;
    };
    let mut count = 1_usize;
    for byte in bytes {
        if byte != marker {
            return false;
        }
        count += 1;
    }
    count >= 3
}

fn is_table_separator(value: &str) -> bool {
    // split 即使对空文本也产生一个单元格；空行仍由长度校验拒绝。
    split_table_row(value).all(|cell| {
        cell.len() >= 3 && cell.chars().all(|character| character == '-' || character == ':')
    })
}

fn split_table_row(value: &str) -> impl Iterator<Item = &str> {
    let value = value.trim();
    let value = value.strip_prefix('|').unwrap_or(value);
    let value = value.strip_suffix('|').unwrap_or(value);
    value.split('|').map(str::trim)
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
