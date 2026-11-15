//! 运维输入的规范文本校验：单行文本、操作者、确认短语与十六进制摘要。

use uuid::Uuid;

use super::digest;

/// 1–`max` 字节、首尾无空白且不含控制字符的单行文本；操作者、恢复点标识、停机原因与
/// 演练回执字段共用同一判定，各调用方保留自己的错误文案。
pub(crate) fn single_line(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

/// 操作者须为 1-128 字节、首尾无空白且不含控制字符的单行文本。
pub(crate) fn actor(value: &str) -> anyhow::Result<&str> {
    if !single_line(value, 128) {
        anyhow::bail!("--actor 必须是 1-128 字节、首尾无空白且不含控制字符的文本");
    }
    Ok(value)
}

/// 写入命令的稳定身份不得为 nil UUID。
pub(crate) fn command_id(value: Uuid, name: &str) -> anyhow::Result<Uuid> {
    if value.is_nil() {
        anyhow::bail!("{name} 必须是非零 UUID");
    }
    Ok(value)
}

/// 确认短语必须与期望值逐字相等，错误信息给出期望格式以便复制。
pub(crate) fn confirmation(actual: &str, expected: &str) -> anyhow::Result<()> {
    if actual != expected {
        anyhow::bail!("确认短语不匹配；必须逐字输入：{expected}");
    }
    Ok(())
}

/// 解析规范小写 SHA-256 十六进制。
pub(crate) fn sha256_hex(value: &str) -> anyhow::Result<[u8; 32]> {
    if !digest::is_lower_hex(value, 64) {
        anyhow::bail!("摘要必须是 64 位小写十六进制 SHA-256");
    }
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::{actor, single_line};

    #[test]
    fn single_line_bounds_length_whitespace_and_control_characters() {
        assert!(single_line("a", 1));
        assert!(single_line(&"a".repeat(128), 128));
        assert!(single_line("ops team", 128), "内部空格允许");
        assert!(!single_line("", 128));
        assert!(!single_line(&"a".repeat(129), 128));
        assert!(!single_line(" ops", 128));
        assert!(!single_line("ops ", 128));
        assert!(!single_line("ops\n", 128));
        assert!(!single_line("o\nps", 128));
        assert!(!single_line("o\u{7}ps", 128));
        // 上限按字节计：两个三字节汉字超过 5 字节上限。
        assert!(single_line("运维", 6));
        assert!(!single_line("运维", 5));
    }

    #[test]
    fn actor_rejects_untrimmed_and_multiline_values() {
        assert_eq!(actor("ops@example").expect("合法操作者"), "ops@example");
        assert!(actor(" ops").is_err());
        assert!(actor("ops\nroot").is_err());
        assert!(actor("").is_err());
    }
}
