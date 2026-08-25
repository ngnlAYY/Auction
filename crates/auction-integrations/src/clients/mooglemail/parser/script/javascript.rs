//! JavaScript 文本遮罩和脚本协议值提取。

use super::*;

pub(crate) fn required_script_value(source: &str, name: &str) -> Option<i64> {
    let needle = format!("var {name}");
    let mut matches = source.match_indices(&needle);
    let first = matches.find(|(index, _)| {
        *index == 0
            || source[..*index]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_whitespace() || character == ';')
    })?;
    if matches.next().is_some() {
        return None;
    }
    let mut index = first.0 + needle.len();
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
        index += 1;
    }
    if source.as_bytes().get(index) != Some(&b'=') {
        return None;
    }
    index += 1;
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
        index += 1;
    }
    let start = index;
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_digit()) {
        index += 1;
    }
    if start == index {
        return None;
    }
    let value = source[start..index].parse::<i64>().ok()?;
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
        index += 1;
    }
    (source.as_bytes().get(index) == Some(&b';')).then_some(value)
}

pub(crate) fn script_text(
    document: &Html,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> Result<String, MoogleMailError> {
    let selector = selector("script", operation, world)?;
    let mut source = String::new();
    for script in document.select(&selector) {
        source.push_str(&script.text().collect::<String>());
        source.push('\n');
    }
    Ok(source)
}

/// 把注释和字符串替换为空格，防止伪造的变量赋值或函数调用进入协议状态。
pub(crate) fn executable_javascript(source: &str) -> Option<String> {
    #[derive(Clone, Copy)]
    enum State {
        Code,
        Single,
        Double,
        Template,
        LineComment,
        BlockComment,
    }
    let mut state = State::Code;
    let mut escaped = false;
    let mut output = source.as_bytes().to_vec();
    let mut index = 0;
    while index < output.len() {
        let byte = source.as_bytes()[index];
        match state {
            State::Code => match byte {
                b'/' if source.as_bytes().get(index + 1) == Some(&b'/') => {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 1;
                    state = State::LineComment;
                }
                b'/' if source.as_bytes().get(index + 1) == Some(&b'*') => {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 1;
                    state = State::BlockComment;
                }
                b'\'' => {
                    output[index] = b' ';
                    state = State::Single;
                }
                b'"' => {
                    output[index] = b' ';
                    state = State::Double;
                }
                b'`' => {
                    output[index] = b' ';
                    state = State::Template;
                }
                _ => {}
            },
            State::LineComment => {
                if byte == b'\n' {
                    state = State::Code;
                } else {
                    output[index] = b' ';
                }
            }
            State::BlockComment => {
                if byte == b'*' && source.as_bytes().get(index + 1) == Some(&b'/') {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 1;
                    state = State::Code;
                } else if byte != b'\n' && byte != b'\r' {
                    output[index] = b' ';
                }
            }
            State::Single | State::Double | State::Template => {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if matches!(
                    (state, byte),
                    (State::Single, b'\'') | (State::Double, b'"') | (State::Template, b'`')
                ) {
                    state = State::Code;
                } else if (byte == b'\n' || byte == b'\r') && !matches!(state, State::Template) {
                    return None;
                }
                if !matches!(byte, b'\n' | b'\r') {
                    output[index] = b' ';
                }
            }
        }
        index += 1;
    }
    matches!(state, State::Code | State::LineComment)
        .then(|| String::from_utf8(output).ok())
        .flatten()
}

pub(crate) fn extract_equipment_id(value: &str) -> Option<String> {
    let marker = "equips.set(";
    let matches = value.match_indices(marker).take(2).collect::<Vec<_>>();
    if matches.len() != 1 {
        return None;
    }
    let (marker_start, _) = matches[0];
    if marker_start > 0
        && !value[..marker_start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_whitespace() || character == ';')
    {
        return None;
    }
    let start = marker_start + marker.len();
    let remainder = value[start..].trim_start();
    let end = remainder.find(|character: char| !character.is_ascii_digit())?;
    let id = &remainder[..end];
    (valid_decimal_id(id) && remainder[end..].trim_start().starts_with(',')).then(|| id.to_owned())
}
