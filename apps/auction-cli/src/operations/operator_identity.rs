//! 只采集 CLI 审计需要的主机身份，不读取或输出其他环境变量。

use auction_application::core::CliOperatorIdentity;

/// 使用内核的有效 UID；sudo 发起者保留为环境声明，不作为授权依据。
pub(super) fn capture() -> anyhow::Result<CliOperatorIdentity> {
    let sudo_user = match std::env::var("SUDO_USER") {
        Ok(value) => {
            if value.is_empty()
                || value.len() > 128
                || value.trim() != value
                || value.chars().any(char::is_control)
            {
                anyhow::bail!("SUDO_USER 必须是 1-128 字节、首尾无空白且不含控制字符的用户名");
            }
            Some(value)
        }
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("SUDO_USER 必须是有效 UTF-8 用户名");
        }
    };
    // SAFETY：geteuid 无参数、无前置条件，不读取可伪造的 UID 环境变量。
    let effective_uid = unsafe { libc::geteuid() };
    Ok(CliOperatorIdentity { effective_uid, sudo_user })
}
