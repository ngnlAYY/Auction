//! MoogleMail 写入页面共用的安全文本、金额和响应定位辅助。

use std::str;

use scraper::{ElementRef, Html, Selector};

use super::model::MAX_BODY_BYTES;
use super::*;

pub(super) fn input_value(
    form: ElementRef<'_>,
    name: &str,
    world: MoogleMailWorld,
) -> Result<String, MoogleMailError> {
    let selector = selector(&format!("input[name='{name}']"), world)?;
    let fields = form.select(&selector).collect::<Vec<_>>();
    if fields.len() != 1 {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok(fields[0].value().attr("value").unwrap_or_default().to_owned())
}

pub(super) fn parse_credit_attachment(value: &str) -> Option<i64> {
    let value = value.strip_suffix(" Credits")?;
    let value = value.strip_suffix('x').unwrap_or(value);
    let amount = parse_amount(value)?;
    (amount > 0).then_some(amount)
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
        Some(parse_amount(amount)?)
    };
    Some((usize::try_from(count).ok()?, cod))
}

pub(super) fn parse_amount(value: &str) -> Option<i64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || value
            .bytes()
            .enumerate()
            .any(|(index, byte)| byte == b',' && (index < 1 || index + 1 >= value.len()))
    {
        return None;
    }
    let groups = value.split(',').collect::<Vec<_>>();
    if groups.len() > 1
        && (groups[0].is_empty()
            || groups[0].len() > 3
            || !groups[0].bytes().all(|byte| byte.is_ascii_digit())
            || groups[0].starts_with('0')
            || groups[1..]
                .iter()
                .any(|group| group.len() != 3 || !group.bytes().all(|byte| byte.is_ascii_digit())))
    {
        return None;
    }
    if groups.len() == 1 && !groups[0].bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.replace(',', "").parse::<i64>().ok()
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
    let mut visible = String::new();
    for node in body.descendants() {
        let Some(text) = node.value().as_text() else {
            continue;
        };
        let hidden = node.ancestors().filter_map(ElementRef::wrap).any(|ancestor| {
            matches!(ancestor.value().name(), "script" | "style" | "noscript" | "textarea")
        });
        if !hidden {
            visible.push_str(text);
            visible.push(' ');
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

pub(super) fn direct_element_children(element: ElementRef<'_>) -> Vec<ElementRef<'_>> {
    element.children().filter_map(ElementRef::wrap).collect()
}

pub(super) fn selector(value: &str, world: MoogleMailWorld) -> Result<Selector, MoogleMailError> {
    Selector::parse(value).map_err(|_| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))
}

pub(super) fn extract_equipment_id(value: &str) -> Option<String> {
    let marker = "equips.set(";
    let matches = value.match_indices(marker).collect::<Vec<_>>();
    if matches.len() != 1 {
        return None;
    }
    let (start, _) = matches[0];
    if start > 0
        && !value[..start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_whitespace() || character == ';')
    {
        return None;
    }
    let remainder = value[start + marker.len()..].trim_start();
    let end = remainder.find(|character: char| !character.is_ascii_digit())?;
    let equipment_id = &remainder[..end];
    (valid_decimal_id(equipment_id) && remainder[end..].trim_start().starts_with(','))
        .then(|| equipment_id.to_owned())
}

pub(super) fn valid_body(value: &str) -> bool {
    value.len() <= MAX_BODY_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

pub(super) fn canonical_body(value: &str) -> String {
    value.replace("\r\n", "\n").trim_end_matches([' ', '\t', '\n']).to_owned()
}

pub(super) fn normalize_text(value: String) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn compose_error(
    world: MoogleMailWorld,
    category: MoogleMailErrorCategory,
) -> MoogleMailError {
    MoogleMailError::new(MoogleMailOperation::Compose, world, category, false)
}
