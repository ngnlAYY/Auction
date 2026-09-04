//! Markdown 行内语法解析与链接安全校验。

use super::model::Inline;

pub(super) fn parse_inline(value: &str) -> Vec<Inline> {
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
        // 允许站内绝对路径（以单个 `/` 开头），但拒绝 `//` 开头的协议相对 URL：
        // 协议相对 URL 会继承当前页面协议并跳转到外部域，从而绕过白名单。
        || (value.starts_with('/') && !value[1..].starts_with('/'))
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
