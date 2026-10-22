//! Markdown 行内语法解析与链接安全校验。

use super::model::Inline;

/// 行内解析递归深度上限：恶意或病态嵌套（如 `[[[[…]]]]`、交错强调标记）会逐层递归，
/// 超限后把剩余内容按纯文本处理，避免栈耗尽或指数级解析开销。
const MAX_INLINE_DEPTH: usize = 8;

/// 单个链接最多扫描 1 KiB 标签和 4 KiB 目标；超限按文本处理，不截断正文。
/// 固定窗口避免大量未闭合 `[` / `<http://` 对同一长后缀反复全量搜索。
const MAX_LINK_LABEL_BYTES: usize = 1024;
const MAX_LINK_TARGET_BYTES: usize = 4096;

pub(super) fn parse_inline(value: &str) -> Vec<Inline> {
    parse_inline_at(value, 0)
}

fn parse_inline_at(value: &str, depth: usize) -> Vec<Inline> {
    if depth >= MAX_INLINE_DEPTH {
        // 超限：不再解析任何行内语法，整段按纯文本返回。
        return vec![Inline::Text(value.to_owned())];
    }

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
            && let Some(source) = safe_link_target(target)
        {
            flush_text(&mut nodes, &mut text);
            nodes.push(Inline::Image { alt: label.to_owned(), source });
            index += consumed;
            continue;
        }

        if let Some((label, target, consumed)) = bracket_link(rest, false)
            && let Some(target) = safe_link_target(target)
        {
            flush_text(&mut nodes, &mut text);
            nodes.push(Inline::Link { label: parse_inline_at(label, depth + 1), target });
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
                let nested = parse_inline_at(&inner[..close], depth + 1);
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
            && let Some(close) = bounded_prefix(rest, MAX_LINK_TARGET_BYTES + 2).find('>')
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

fn bracket_link(value: &str, image: bool) -> Option<(&str, &str, usize)> {
    let prefix_length = if image { 2 } else { 1 };
    if image {
        if !value.starts_with("![") {
            return None;
        }
    } else if !value.starts_with('[') {
        return None;
    }
    let label_and_close = bounded_prefix(&value[prefix_length..], MAX_LINK_LABEL_BYTES + 2);
    let close_label = label_and_close.find("](")? + prefix_length;
    let target_start = close_label + 2;
    let close_target =
        bounded_prefix(&value[target_start..], MAX_LINK_TARGET_BYTES + 1).find(')')? + target_start;
    Some((&value[prefix_length..close_label], &value[target_start..close_target], close_target + 1))
}

/// 只取完整 UTF-8 字符边界；窗口结束处的多字节字符按普通文本留给主循环处理。
fn bounded_prefix(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn safe_link_target(value: &str) -> Option<String> {
    // 浏览器会把反斜杠当作斜杠，并移除 URL 内的 Tab/换行；必须在 trim 前拒绝原始目标。
    if has_unsafe_target_character(value) {
        return None;
    }
    let value = value.trim();
    let allowed = value.get(..8).is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        || value.get(..7).is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || value.get(..7).is_some_and(|prefix| prefix.eq_ignore_ascii_case("mailto:"))
        || is_internal_target(value);
    allowed.then(|| value.to_owned())
}

/// 与目标白名单共用站内判断；任何反斜杠或控制字符都不能成为站内导航。
pub(super) fn is_internal_target(target: &str) -> bool {
    !has_unsafe_target_character(target)
        && ((target.starts_with('/') && !target.starts_with("//"))
            || target.starts_with("./")
            || target.starts_with("../")
            || target.starts_with('#')
            || target.starts_with('?'))
}

fn has_unsafe_target_character(target: &str) -> bool {
    target.chars().any(|character| character == '\\' || character.is_control())
}

fn flush_text(nodes: &mut Vec<Inline>, text: &mut String) {
    if !text.is_empty() {
        nodes.push(Inline::Text(std::mem::take(text)));
    }
}
