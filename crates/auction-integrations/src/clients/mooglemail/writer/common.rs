//! MoogleMail 写入页面共用的安全文本、金额和响应定位辅助。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::str;

use scraper::{ElementRef, Html, Node, Selector};

use super::*;
use crate::clients::mooglemail::parser::{
    direct_element_children, normalize_text, parse_canonical_credits, unique_element,
};

pub(super) fn input_value(
    form: ElementRef<'_>,
    name: &str,
    world: MoogleMailWorld,
) -> Result<String, MoogleMailError> {
    let selector = selector(&format!("input[name='{name}']"), world)?;
    let field = unique_element(form.select(&selector), MoogleMailOperation::Compose, world)?;
    Ok(field.value().attr("value").unwrap_or_default().to_owned())
}

pub(super) fn parse_attachment_count(value: &str) -> Option<(usize, Option<i64>)> {
    let (prefix, suffix) = value.split_once(" items attached")?;
    let (count, maximum) = prefix.split_once(" / ")?;
    let count = parse_unsigned(count)?;
    let maximum = parse_unsigned(maximum)?;
    if count > maximum {
        return None;
    }
    let cod = if suffix.is_empty() {
        None
    } else {
        let amount = suffix.strip_prefix(" - CoD: ").or_else(|| suffix.strip_prefix(" – CoD: "))?;
        Some(parse_canonical_credits(amount)?)
    };
    Some((usize::try_from(count).ok()?, cod))
}

/// 读取 compose 附件计数；兼容当前页面的固定 CSS 字形结构。
pub(super) fn attachment_count_text(element: ElementRef<'_>) -> Option<String> {
    let visible = normalize_text(element.text().collect::<String>());
    if !visible.is_empty() {
        return Some(visible);
    }

    let wrappers = direct_element_children(element);
    if wrappers.len() != 1
        || wrappers[0].value().name() != "div"
        || wrappers[0].value().attr("class") != Some("fc f4b")
        || !normalize_text(wrappers[0].text().collect::<String>()).is_empty()
    {
        return None;
    }
    let glyphs = direct_element_children(wrappers[0]);
    if glyphs.is_empty() {
        return None;
    }
    let mut decoded = String::with_capacity(glyphs.len());
    for glyph in glyphs {
        if glyph.value().name() != "div"
            || !direct_element_children(glyph).is_empty()
            || !normalize_text(glyph.text().collect::<String>()).is_empty()
        {
            return None;
        }
        let class_name = glyph.value().attr("class")?;
        let bytes = class_name.as_bytes();
        match bytes {
            [b'c', b'5', b'9'] => decoded.push(' '),
            [b'c', b'4', b'i'] => decoded.push('/'),
            [b'c', b'4', digit @ b'0'..=b'9'] => decoded.push(char::from(*digit)),
            [b'c', b'5', letter @ b'a'..=b'z'] => decoded.push(char::from(*letter)),
            _ => return None,
        }
    }
    let decoded = normalize_text(decoded);
    parse_attachment_count(&decoded).is_some().then_some(decoded)
}

pub(super) fn parse_unsigned(value: &str) -> Option<u64> {
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) {
        return None;
    }
    value.bytes().all(|byte| byte.is_ascii_digit()).then(|| value.parse().ok()).flatten()
}

pub(super) fn visible_confirmation_text(
    element: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<String, MoogleMailError> {
    let text = normalize_text(element.text().collect::<String>());
    if !text.is_empty() {
        return Ok(text);
    }
    let selector = selector("div", world)?;
    let mut decoded = String::new();
    for child in element.select(&selector) {
        let classes = child.value().attr("class").unwrap_or_default().split_whitespace();
        for class_name in classes {
            let bytes = class_name.as_bytes();
            if bytes.len() != 3 || bytes[0] != b'c' {
                continue;
            }
            match (bytes[1], bytes[2]) {
                (b'3' | b'5', b'9') => decoded.push(' '),
                (b'3' | b'5', b'a'..=b'z') => decoded.push(char::from(bytes[2])),
                (b'2' | b'4', b'0'..=b'9') => decoded.push(char::from(bytes[2])),
                _ => {}
            }
        }
    }
    Ok(normalize_text(decoded))
}

/// 当前 HentaiVerse 也会用单一 CSS sprite 表示“邮件已发送”。
/// 该提示只在调用方已经确认唯一 `#mmail_fin` 后使用；这里再要求完整的直接子结构，
/// 避免任意 `c5y` class 充当提示。最终成功仍须独立完成 Sent 与空 compose 证明。
pub(super) fn is_css_send_confirmation(element: ElementRef<'_>) -> bool {
    if !normalize_text(element.text().collect::<String>()).is_empty() {
        return false;
    }
    let wrappers = direct_element_children(element);
    if wrappers.len() != 1
        || wrappers[0].value().name() != "div"
        || wrappers[0].value().attr("class") != Some("fc f4b")
    {
        return false;
    }
    let markers = direct_element_children(wrappers[0]);
    markers.len() == 1
        && markers[0].value().name() == "div"
        && markers[0].value().attr("class") == Some("c5y")
        && direct_element_children(markers[0]).is_empty()
        && normalize_text(markers[0].text().collect::<String>()).is_empty()
}

pub(super) fn response_mail_id(source: &str) -> Option<String> {
    let bytes = source.as_bytes();
    let mut offset = 0;
    let mut found = None;
    while let Some(relative) = source[offset..].find("mid=") {
        let start = offset + relative;
        let previous = start.checked_sub(1).and_then(|index| bytes.get(index).copied());
        let valid_prefix = matches!(previous, Some(b'?' | b'&'))
            || previous == Some(b';') && source[..start].ends_with("&amp;");
        if !valid_prefix {
            offset = start.saturating_add(4);
            continue;
        }
        let value_start = start + 4;
        let mut value_end = value_start;
        while bytes.get(value_end).is_some_and(|byte| byte.is_ascii_digit()) {
            value_end += 1;
        }
        if value_start == value_end {
            return None;
        }
        if bytes.get(value_end).is_some_and(|byte| {
            !matches!(*byte, b'&' | b'#' | b'\'' | b'"') && !byte.is_ascii_whitespace()
        }) {
            return None;
        }
        let value = &source[value_start..value_end];
        if !valid_decimal_id(value) {
            return None;
        }
        if found.replace(value).is_some() {
            return None;
        }
        offset = value_end;
    }
    found.map(str::to_owned)
}

pub(super) fn visible_body_text(body: ElementRef<'_>) -> String {
    // 提示区自身或其外层处于隐藏元素时，任何后代文字都不能作为拒绝证据。
    if body.ancestors().filter_map(ElementRef::wrap).any(|ancestor| {
        matches!(ancestor.value().name(), "script" | "style" | "noscript" | "textarea")
    }) {
        return String::new();
    }
    let mut visible = String::new();
    let mut ancestors = Vec::new();
    // 深层嵌套只遍历一次祖先栈，保持隐藏诱饵和可见文本的原有边界。
    for node in body.descendants() {
        let parent = node.parent().map(|parent| parent.id());
        while ancestors.last().is_some_and(|(id, _)| Some(*id) != parent) {
            ancestors.pop();
        }
        let hidden = ancestors.last().is_some_and(|(_, hidden)| *hidden)
            || matches!(node.value(), Node::Element(element)
                if matches!(element.name(), "script" | "style" | "noscript" | "textarea"));
        match node.value() {
            Node::Element(_) => ancestors.push((node.id(), hidden)),
            Node::Text(text) if !hidden => {
                visible.push_str(text);
                visible.push(' ');
            }
            _ => {}
        }
    }
    normalize_text(visible)
}

pub(super) fn has_visible_result_message(
    document: &Html,
    world: MoogleMailWorld,
) -> Result<bool, MoogleMailError> {
    let selector = selector("#messagebox_inner > p", world)?;
    Ok(document
        .select(&selector)
        .any(|paragraph| !normalize_text(paragraph.text().collect::<String>()).is_empty()))
}

pub(super) fn selector(value: &str, world: MoogleMailWorld) -> Result<Selector, MoogleMailError> {
    Selector::parse(value).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_send",
            "build_compose_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        compose_error(world, MoogleMailErrorCategory::ProtocolDrift)
    })
}

pub(super) fn compose_error(
    world: MoogleMailWorld,
    category: MoogleMailErrorCategory,
) -> MoogleMailError {
    MoogleMailError::new(MoogleMailOperation::Compose, world, category, false)
}
