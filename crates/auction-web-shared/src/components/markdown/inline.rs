//! Markdown 行内语法解析与链接安全校验。

use super::model::Inline;

/// 行内解析递归深度上限：恶意或病态嵌套（如 `[[[[…]]]]`、交错强调标记）会逐层递归，
/// 超限后把剩余内容按纯文本处理，避免栈耗尽或指数级解析开销。
const MAX_INLINE_DEPTH: usize = 8;

/// 单个链接最多接受 1 KiB 标签和 4 KiB 目标；超限按文本处理，不截断正文。
/// 记住下一个闭合符的位置，避免大量未闭合 `[` / `<http://` 重复扫描重叠窗口。
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
    let mut delimiters = LinkDelimiters::default();

    while index < value.len() {
        let rest = &value[index..];
        if rest.starts_with('\\')
            && let Some(character) = rest[1..].chars().next()
        {
            text.push(character);
            index += 1 + character.len_utf8();
            continue;
        }

        if let Some((label, target, consumed)) = bracket_link(value, index, true, &mut delimiters)
            && let Some(source) = safe_link_target(target)
        {
            flush_text(&mut nodes, &mut text);
            nodes.push(Inline::Image { alt: label.to_owned(), source });
            index += consumed;
            continue;
        }

        if let Some((label, target, consumed)) = bracket_link(value, index, false, &mut delimiters)
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
            && let Some(close) = next_delimiter(value, index + 1, ">", &mut delimiters.autolink)
            && close - index - 1 <= MAX_LINK_TARGET_BYTES
            && let Some(target) = safe_link_target(&value[index + 1..close])
        {
            flush_text(&mut nodes, &mut text);
            nodes.push(Inline::Link {
                label: vec![Inline::Text(value[index + 1..close].to_owned())],
                target,
            });
            index = close + 1;
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

#[derive(Default)]
struct LinkDelimiters {
    label: Option<usize>,
    target: Option<usize>,
    autolink: Option<usize>,
}

fn bracket_link<'a>(
    value: &'a str,
    start: usize,
    image: bool,
    delimiters: &mut LinkDelimiters,
) -> Option<(&'a str, &'a str, usize)> {
    let rest = &value[start..];
    let prefix_length = if image { 2 } else { 1 };
    if image {
        if !rest.starts_with("![") {
            return None;
        }
    } else if !rest.starts_with('[') {
        return None;
    }
    let label_start = start + prefix_length;
    let close_label = next_delimiter(value, label_start, "](", &mut delimiters.label)?;
    if close_label - label_start > MAX_LINK_LABEL_BYTES {
        return None;
    }
    let target_start = close_label + 2;
    let close_target = next_delimiter(value, target_start, ")", &mut delimiters.target)?;
    if close_target - target_start > MAX_LINK_TARGET_BYTES {
        return None;
    }
    Some((
        &value[label_start..close_label],
        &value[target_start..close_target],
        close_target + 1 - start,
    ))
}

/// 主循环只向前移动；缓存的结束位置随起点前进，缺失时以字符串末尾作为哨兵。
fn next_delimiter(
    value: &str,
    start: usize,
    delimiter: &str,
    cached: &mut Option<usize>,
) -> Option<usize> {
    if (*cached).is_none_or(|position| position < start) {
        *cached = Some(value[start..].find(delimiter).map_or(value.len(), |offset| start + offset));
    }
    (*cached).filter(|position| *position < value.len())
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
