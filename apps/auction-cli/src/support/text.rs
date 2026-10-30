//! 运维输入的规范文本校验：操作者、确认短语与十六进制摘要。

use uuid::Uuid;

/// 操作者须为 1-128 字节、首尾无空白且不含控制字符的单行文本。
pub(crate) fn actor(value: &str) -> anyhow::Result<&str> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
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
    if value.len() != 64 || !value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        anyhow::bail!("摘要必须是 64 位小写十六进制 SHA-256");
    }
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(digest)
}
