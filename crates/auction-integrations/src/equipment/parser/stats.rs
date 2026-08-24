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
    for byte in value.as_bytes()[1..].iter().copied() {
        match byte {
            b'0'..=b'9' => {
                digits += 1;
                normalized.push(byte as char);
            }
            b',' if !dot_seen => {}
            b'.' if !dot_seen => {
                dot_seen = true;
                normalized.push('.');
            }
            _ => return None,
        }
    }
    (digits > 0).then_some(normalized)
}
