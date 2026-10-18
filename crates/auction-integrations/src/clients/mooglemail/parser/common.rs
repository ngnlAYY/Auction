use super::*;

/// Sent 列表把标题截断为前缀；详情标题必须等于或以该列表标题开头。
pub(crate) fn subject_is_list_projection(list_subject: &str, detail_subject: &str) -> bool {
    !list_subject.is_empty()
        && list_subject.len() <= detail_subject.len()
        && detail_subject.starts_with(list_subject)
}

pub(crate) fn parse_detail_actions(
    envelope: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<(bool, bool), MoogleMailError> {
    let selector = selector("[onclick]", MoogleMailOperation::Detail, world)?;
    let mut returns = 0;
    let mut claims = 0;
    for action in envelope.select(&selector) {
        let value = action.value().attr("onclick").unwrap_or_default();
        let normalized =
            value.chars().filter(|character| !character.is_whitespace()).collect::<String>();
        if matches!(normalized.as_str(), "mooglemail.return_mail()" | "mooglemail.return_mail();") {
            returns += 1;
        } else if normalized.contains("return_mail") {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        if matches!(
            normalized.as_str(),
            "mooglemail.remove_attachment(0)" | "mooglemail.remove_attachment(0);"
        ) {
            claims += 1;
        } else if normalized.contains("remove_attachment") {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
    }
    if returns > 1 || claims > 1 {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok((returns == 1, claims == 1))
}

pub(crate) fn attribute_references_mail_id(value: &str, external_mail_id: &str) -> bool {
    let bytes = value.as_bytes();
    for (index, _) in value.match_indices("mid=") {
        if index != 0 && !matches!(bytes[index - 1], b'?' | b'&') {
            continue;
        }
        let start = index + 4;
        let end = value[start..]
            .find(|character: char| {
                matches!(character, '&' | '#' | '\'' | '"') || character.is_whitespace()
            })
            .map_or(value.len(), |offset| start + offset);
        if start == end {
            return true;
        }
        let Some(decoded) = decode_form_component(&value[start..end]) else {
            return true;
        };
        if !valid_opaque_id(&decoded) || decoded == external_mail_id {
            return true;
        }
    }
    false
}

pub(crate) fn extract_mid(value: &str) -> Option<String> {
    let mut found = None;
    let bytes = value.as_bytes();
    for (index, _) in value.match_indices("mid=") {
        if index != 0 && !matches!(bytes[index - 1], b'?' | b'&') {
            continue;
        }
        if found.is_some() {
            return None;
        }
        let start = index + 4;
        let end = value[start..]
            .find(|character: char| {
                matches!(character, '&' | '#' | '\'' | '"') || character.is_whitespace()
            })
            .map_or(value.len(), |offset| start + offset);
        if start == end {
            return None;
        }
        let encoded = format!("mid={}", &value[start..end]);
        let decoded = decode_form_component(encoded.strip_prefix("mid=")?)?;
        if !valid_opaque_id(&decoded) {
            return None;
        }
        found = Some(decoded);
    }
    found
}

pub(crate) fn decode_form_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' => {
                let high = hex_digit(*bytes.get(index + 1)?)?;
                let low = hex_digit(*bytes.get(index + 2)?)?;
                decoded.push((high << 4) | low);
                index += 2;
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8(decoded).ok()
}

pub(crate) fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn parse_page_time(
    value: &str,
    operation: MoogleMailOperation,
    world: MoogleMailWorld,
) -> Result<DateTime<Utc>, MoogleMailError> {
    let naive = NaiveDateTime::parse_from_str(value, INBOX_TIME_FORMAT)
        .map_err(|_| protocol(operation, world))?;
    let offset = FixedOffset::west_opt(8 * 60 * 60).ok_or_else(|| protocol(operation, world))?;
    offset
        .from_local_datetime(&naive)
        .single()
        .map(|value| value.with_timezone(&Utc))
        .ok_or_else(|| protocol(operation, world))
}

pub(crate) fn valid_message_body(value: &str) -> bool {
    value.len() <= MAX_MESSAGE_TEXT_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

pub(crate) fn normalize_text(value: String) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn direct_children<'a>(element: ElementRef<'a>, tag: &str) -> Vec<ElementRef<'a>> {
    element
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|child| child.value().name() == tag)
        .collect()
}

pub(crate) fn direct_element_children(element: ElementRef<'_>) -> Vec<ElementRef<'_>> {
    element.children().filter_map(ElementRef::wrap).collect()
}

/// 解码 HentaiVerse 固定标签使用的受限 CSS 字形结构。
/// 字形只允许作为指定 wrapper 的直接空 `div` 子元素出现；`c3x` 表示单个小写字母，
/// 详情标签额外要求一个结尾 `c2m` 冒号。调用方仍须把结果与自己的固定闭集逐字核对。
pub(crate) fn decode_css_glyph_label(
    wrapper: ElementRef<'_>,
    expected_wrapper_class: &str,
    trailing_colon: bool,
) -> Option<String> {
    if wrapper.value().name() != "div"
        || wrapper.value().attr("class") != Some(expected_wrapper_class)
        || !normalize_text(wrapper.text().collect::<String>()).is_empty()
    {
        return None;
    }
    let mut glyphs = direct_element_children(wrapper);
    if trailing_colon {
        let colon = glyphs.pop()?;
        if colon.value().name() != "div"
            || colon.value().attr("class") != Some("c2m")
            || !direct_element_children(colon).is_empty()
            || !normalize_text(colon.text().collect::<String>()).is_empty()
        {
            return None;
        }
    }
    if glyphs.is_empty()
        || glyphs.iter().any(|glyph| {
            glyph.value().name() != "div"
                || !direct_element_children(*glyph).is_empty()
                || !normalize_text(glyph.text().collect::<String>()).is_empty()
        })
    {
        return None;
    }
    let mut decoded = String::with_capacity(glyphs.len());
    for glyph in glyphs {
        let class = glyph.value().attr("class")?;
        if class.split_ascii_whitespace().count() != 1 {
            return None;
        }
        let suffix = class.strip_prefix("c3")?;
        if suffix.len() != 1 || !suffix.as_bytes()[0].is_ascii_lowercase() {
            return None;
        }
        decoded.push(char::from(suffix.as_bytes()[0]));
    }
    // 远端 glyph class 只编码字母，不编码固定标签的首字母大小写。
    let mut bytes = decoded.into_bytes();
    bytes[0] = bytes[0].to_ascii_uppercase();
    String::from_utf8(bytes).ok()
}

pub(crate) fn selector(
    value: &str,
    operation: MoogleMailOperation,
    world: MoogleMailWorld,
) -> Result<Selector, MoogleMailError> {
    Selector::parse(value).map_err(|_| protocol(operation, world))
}

pub(crate) fn protocol(operation: MoogleMailOperation, world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(operation, world, MoogleMailErrorCategory::ProtocolDrift, false)
}

pub(crate) fn detail_error(
    world: MoogleMailWorld,
    category: MoogleMailErrorCategory,
) -> MoogleMailError {
    detail_error_for(MoogleMailOperation::Detail, world, category)
}

pub(crate) fn detail_error_for(
    operation: MoogleMailOperation,
    world: MoogleMailWorld,
    category: MoogleMailErrorCategory,
) -> MoogleMailError {
    MoogleMailError::new(operation, world, category, false)
}

pub(crate) fn action_proof_error(
    operation: MoogleMailOperation,
    world: MoogleMailWorld,
) -> MoogleMailError {
    detail_error_for(operation, world, MoogleMailErrorCategory::WriteUnknown)
}

pub(crate) fn write_digest_string(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
}
