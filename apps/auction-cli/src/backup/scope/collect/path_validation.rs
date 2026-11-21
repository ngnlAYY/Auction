//! 备份范围路径各段的格式校验。

use super::MAX_RELATIVE_BYTES;

/// 相对路径按段校验：拒绝空段、`.`、`..`、反斜杠、控制字符与首尾空白。
pub(crate) fn validate_relative(path: &str) -> anyhow::Result<()> {
    if path.is_empty() || path.len() > MAX_RELATIVE_BYTES {
        anyhow::bail!("备份范围相对路径无效");
    }
    for part in path.split('/') {
        validate_component(part)?;
    }
    Ok(())
}

pub(super) fn validate_component(name: &str) -> anyhow::Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
        || name.chars().any(char::is_control)
        || name.trim() != name
    {
        anyhow::bail!("备份范围名称无效：{name:?}");
    }
    Ok(())
}
