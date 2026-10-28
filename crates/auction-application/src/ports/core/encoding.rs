//! 摘要、指纹、确认短语和旧版 JSON 共用的稳定编码。

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
