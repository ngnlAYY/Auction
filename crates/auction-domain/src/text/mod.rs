//! 跨层共用的规范文本不变量。

/// 规范单行文本：非空、无首尾空白、不含任何控制字符，且 UTF-8 字节数不超过 `maximum`。
/// 命令、端口、持久化映射与外部协议校验共用同一规则，避免各层手写副本漂移。
pub fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

/// 规范正十进制标识：1 至 `maximum` 个 ASCII 数字且无前导零；论坛主题、帖子、账号等外部 ID 共用。
pub fn valid_positive_decimal(value: &str, maximum: usize) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= maximum
        && bytes[0] != b'0'
        && bytes.iter().all(u8::is_ascii_digit)
}
