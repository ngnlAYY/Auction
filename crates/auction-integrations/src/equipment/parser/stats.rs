//! 装备统计值的规范化。

/// 将带正负号、千位分隔符和小数点的统计值规范为 ASCII 数字字符串。
pub(crate) fn normalize_stat_value(value: &str) -> Option<String> {
    let value = value.trim();
    let first = value.as_bytes().first().copied()?;
    if !matches!(first, b'+' | b'-') || value.len() == 1 {
        return None;
    }
    let mut dot_seen = false;
    let mut digits = 0;
    let mut normalized = String::with_capacity(value.len());
    normalized.push(first as char);
    let bytes = value.as_bytes();
    for index in 1..bytes.len() {
        match bytes[index] {
            byte @ b'0'..=b'9' => {
                digits += 1;
                normalized.push(byte as char);
            }
            // 千位分隔符只能出现在整数部分：前面有数字、后面恰好三位数字，再后面不得是数字。
            b',' if !dot_seen => {
                let rest = &bytes[index + 1..];
                let grouped = rest.len() >= 3
                    && rest[..3].iter().all(u8::is_ascii_digit)
                    && rest.get(3).is_none_or(|byte| !byte.is_ascii_digit())
                    && bytes[index - 1].is_ascii_digit();
                if !grouped {
                    return None;
                }
            }
            b'.' if !dot_seen => {
                dot_seen = true;
                normalized.push('.');
            }
            _ => return None,
        }
    }
    (digits > 0).then_some(normalized)
}
