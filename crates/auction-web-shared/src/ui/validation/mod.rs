//! 浏览器 wire 字段的通用格式谓词；两端 DTO 解码与路由参数共用，不承载业务规则。

/// 旧版浏览器 decoder 接受的 UUID：36 字节小写连字符规范形式、版本 1 至 5、RFC 4122 variant。
/// 逐字节判断，不解析也不分配；nil UUID 的版本为 0，因此同样被拒绝。
pub fn valid_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
            }
        })
        && matches!(bytes[14], b'1'..=b'5')
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}
