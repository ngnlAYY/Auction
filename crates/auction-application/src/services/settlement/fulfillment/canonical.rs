//! 旧版 Go JSON 字节兼容编码。

use serde::Serialize;

/// 按 Go `encoding/json.Marshal` 的默认 HTML 安全规则编码稳定 JSON。
///
/// serde 与 Go 对结构字段顺序、整数和有效 UTF-8 字符串的编码一致，但 Go 还会默认转义
/// `<`、`>`、`&`、U+2028 与 U+2029。履约摘要绑定的是原始字节，因此这些差异不能只在
/// 语义 JSON 层比较。
pub(super) fn to_go_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let encoded = serde_json::to_vec(value)?;
    let mut result = Vec::with_capacity(encoded.len());
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
            result.extend_from_slice(replacement);
            index += consumed;
        } else {
            result.push(encoded[index]);
            index += 1;
        }
    }
    Ok(result)
}
