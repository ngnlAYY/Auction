//! 摘要、指纹、确认短语、旧版 JSON 与 URI 转义共用的稳定编码。

use std::borrow::Cow;

/// 编码为固定两位小写十六进制；摘要、指纹与人工确认短语依赖该格式逐字节稳定。
pub fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// 严格解码 URI 百分号转义：`%` 后必须是两位十六进制，结果必须是 UTF-8，不把 `+` 当作空格。
/// 没有转义时借用原文。长度与控制字符边界由调用方按各自语义判定；失败不携带原文。
pub fn strict_percent_decode(value: &str) -> Option<Cow<'_, str>> {
    if !value.as_bytes().contains(&b'%') {
        return Some(Cow::Borrowed(value));
    }
    let hex_digit =
        |byte: u8| char::from(byte).to_digit(16).and_then(|digit| u8::try_from(digit).ok());
    let mut bytes = value.bytes();
    let mut decoded = Vec::with_capacity(value.len());
    while let Some(byte) = bytes.next() {
        decoded.push(if byte == b'%' {
            let high = bytes.next().and_then(hex_digit)?;
            let low = bytes.next().and_then(hex_digit)?;
            (high << 4) | low
        } else {
            byte
        });
    }
    String::from_utf8(decoded).ok().map(Cow::Owned)
}

/// 为 serde 已编码的 JSON 追加 Go 默认 HTML 转义，按原顺序消费借用片段。
/// 原片段与固定替换字节直接交给摘要器或输出缓冲；不另建完整转义副本。
pub(crate) fn for_each_legacy_go_json_segment(encoded: &[u8], mut consume: impl FnMut(&[u8])) {
    let mut start = 0;
    let mut index = 0;
    while index < encoded.len() {
        let replacement = match encoded[index] {
            b'<' => Some((br"\u003c".as_slice(), 1)),
            b'>' => Some((br"\u003e".as_slice(), 1)),
            b'&' => Some((br"\u0026".as_slice(), 1)),
            0xe2 if encoded.get(index..index + 3) == Some([0xe2, 0x80, 0xa8].as_slice()) => {
                Some((br"\u2028".as_slice(), 3))
            }
            0xe2 if encoded.get(index..index + 3) == Some([0xe2, 0x80, 0xa9].as_slice()) => {
                Some((br"\u2029".as_slice(), 3))
            }
            _ => None,
        };
        if let Some((replacement, consumed)) = replacement {
            consume(&encoded[start..index]);
            consume(replacement);
            index += consumed;
            start = index;
        } else {
            index += 1;
        }
    }
    consume(&encoded[start..]);
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{lower_hex, strict_percent_decode};

    #[test]
    fn lower_hex_is_two_lowercase_digits_per_byte() {
        assert_eq!(lower_hex(&[]), "");
        assert_eq!(lower_hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
    }

    #[test]
    fn plain_text_is_borrowed_and_plus_is_literal() {
        assert!(matches!(strict_percent_decode("a+b c"), Some(Cow::Borrowed("a+b c"))));
        assert!(matches!(strict_percent_decode(""), Some(Cow::Borrowed(""))));
    }

    #[test]
    fn escapes_decode_in_either_hex_case() {
        assert_eq!(strict_percent_decode("p%40ss%2Fw%2fd").as_deref(), Some("p@ss/w/d"));
        assert_eq!(strict_percent_decode("%E6%8B%8D%e5%8d%96").as_deref(), Some("拍卖"));
        // 解码只处理一层；控制字符是否允许由调用方判定。
        assert_eq!(strict_percent_decode("%2541%0A").as_deref(), Some("%41\n"));
    }

    #[test]
    fn truncated_non_hex_and_non_utf8_escapes_are_rejected() {
        for value in ["%", "%4", "a%", "%zz", "%4g", "%+1", "%ff", "%E6%8B"] {
            assert_eq!(strict_percent_decode(value), None, "value={value:?}");
        }
    }
}
