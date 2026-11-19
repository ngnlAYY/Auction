//! 受限 HTTPS 地址校验：备份打卡、Worker 外部心跳与运维告警 webhook 共用同一 URL 安全边界。

use url::Url;

use crate::settings::errors::ConfigError;

/// 只接受不超过 4096 字节、无首尾空白与控制字符、无 userinfo 与 fragment 的绝对 HTTPS 地址；
/// 错误由调用方按字段构造，任何分支都不回显配置值。
pub(crate) fn validate_restricted_https_url(
    error: impl Fn() -> ConfigError,
    value: &str,
) -> Result<(), ConfigError> {
    if value.len() > 4096 || value.trim() != value || value.chars().any(char::is_control) {
        return Err(error());
    }
    let url = Url::parse(value).map_err(|_| error())?;
    // 空 userinfo 也拒绝，防止解析器规范化后隐藏原输入中的认证段。
    let authority = value
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default());
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || authority.is_none_or(|authority| authority.contains('@'))
        || url.fragment().is_some()
    {
        return Err(error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::validate_restricted_https_url;
    use crate::settings::errors::ConfigError;

    fn check(value: &str) -> Result<(), ConfigError> {
        validate_restricted_https_url(
            || ConfigError::InvalidBackupHeartbeatUrl { field: "probe_url" },
            value,
        )
    }

    #[test]
    fn restricted_https_url_rejects_hidden_credentials_without_echoing_value() {
        assert!(check("https://hc.example.invalid/ping/abc?run=1").is_ok());
        let oversized = format!("https://hc.example.invalid/{}", "a".repeat(4096));
        for value in [
            "http://hc.example.invalid/ping/secret-token",
            "https:hc.example.invalid/ping/secret-token",
            "https://user@hc.example.invalid/ping/secret-token",
            // 空 userinfo 经解析器规范化后会消失，必须按原文拒绝。
            "https://@hc.example.invalid/ping/secret-token",
            "https://user:pass@hc.example.invalid/ping/secret-token",
            "https://hc.example.invalid/ping/secret-token#fragment",
            " https://hc.example.invalid/ping/secret-token",
            "https://hc.example.invalid/ping/secret-token\n",
            "secret-token",
            oversized.as_str(),
        ] {
            let error = check(value).expect_err(value);
            assert_matches!(
                error,
                ConfigError::InvalidBackupHeartbeatUrl { field: "probe_url" },
                "{value}"
            );
            assert!(!error.to_string().contains("secret-token"), "{value}");
        }
    }
}
