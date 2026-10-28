//! 摘要、指纹和确认短语共用的稳定文本编码。

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
