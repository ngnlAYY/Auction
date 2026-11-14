//! JavaScript 文本遮罩和脚本协议值提取。

use crate::clients::mooglemail::parser::selector;
use crate::clients::mooglemail::protocol::valid_decimal_id;
use crate::clients::mooglemail::{MoogleMailError, MoogleMailOperation, MoogleMailWorld};
use scraper::Html;

pub(crate) fn required_script_value(source: &str, name: &str) -> Option<i64> {
    // 页面既可能逐行 `var x = 0;`，也可能使用 let/const、逗号声明或有符号整数。
    // 任何重复赋值、非整数右值或复合改写都会使状态不再唯一，不能忽略后沿用较早的整数。
    let mut found = None;
    for (index, _) in source.match_indices(name) {
        let before = source[..index].chars().next_back();
        if before
            .is_some_and(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
        {
            continue;
        }
        let mut cursor = index + name.len();
        let after = source[cursor..].chars().next();
        if after
            .is_some_and(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
        {
            continue;
        }
        while source.as_bytes().get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let prefix = source[..index].trim_end();
        let suffix = &source[cursor..];
        if prefix.ends_with("++")
            || prefix.ends_with("--")
            || [
                "++", "--", "+=", "-=", "*=", "/=", "%=", "**=", "<<=", ">>=", ">>>=", "&=", "^=",
                "|=", "&&=", "||=", "??=",
            ]
            .iter()
            .any(|operator| suffix.starts_with(operator))
        {
            return None;
        }
        if !suffix.starts_with('=') || suffix.starts_with("==") {
            continue;
        }
        if prefix.ends_with('.') || found.is_some() {
            return None;
        }
        cursor += 1;
        while source.as_bytes().get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let negative = matches!(source.as_bytes().get(cursor), Some(b'-' | b'+'));
        if negative {
            cursor += 1;
        }
        let start = cursor;
        while source.as_bytes().get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if start == cursor {
            return None;
        }
        let value = source[start..cursor].parse::<i64>().ok()?;
        let value = if negative && source.as_bytes().get(start - 1) == Some(&b'-') {
            value.checked_neg()?
        } else {
            value
        };
        while source.as_bytes().get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let terminator = source.as_bytes().get(cursor).copied();
        if !matches!(terminator, None | Some(b';' | b',' | b'}')) {
            return None;
        }
        found = Some(value);
    }
    found
}

pub(crate) fn script_text(
    document: &Html,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> Result<String, MoogleMailError> {
    let selector = selector("script", operation, world)?;
    let mut source = String::new();
    for script in document.select(&selector) {
        source.extend(script.text());
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
    let mut matches = value.match_indices(marker);
    let (marker_start, _) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
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
