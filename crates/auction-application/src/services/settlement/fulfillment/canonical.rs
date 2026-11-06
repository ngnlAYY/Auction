//! 旧版 Go JSON 字节兼容编码。

use serde::Serialize;

use crate::ports::core::for_each_legacy_go_json_segment;

/// 按 Go `encoding/json.Marshal` 的默认 HTML 安全规则编码稳定 JSON。
/// serde 与 Go 对结构字段顺序、整数和有效 UTF-8 字符串的编码一致，但 Go 还会默认转义
/// `<`、`>`、`&`、U+2028 与 U+2029。履约摘要绑定的是原始字节，因此这些差异不能只在
/// 语义 JSON 层比较。
pub(super) fn to_go_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let encoded = serde_json::to_vec(value)?;
    let mut result = Vec::with_capacity(encoded.len());
    for_each_legacy_go_json_segment(&encoded, |segment| result.extend_from_slice(segment));
    Ok(result)
}
