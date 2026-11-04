//! 异机副本的公开目的地与敏感凭据校验；错误只包含字段名和固定原因。

use serde::{Deserialize, Serialize};

use super::{AppConfig, ConfigError};

/// R2 目的地与 age 公钥；没有私钥、口令或环境凭据入口。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackupOffsiteConfig {
    /// 默认关闭；下次定时运行读取，无需重启角色。
    pub enabled: bool,
    /// age X25519 收件人公钥。
    pub age_recipient: String,
    /// 无认证段、查询或 fragment 的 HTTPS S3 API 地址。
    pub endpoint: String,
    /// 仅此 bucket 的对象读写令牌对应的 bucket 名称。
    pub bucket: String,
    /// 不含空段、点段或首尾斜杠的对象前缀。
    pub prefix: String,
}

pub(super) fn validate(config: &AppConfig) -> Result<(), ConfigError> {
    let offsite = &config.backup.offsite;
    let invalid = |field, reason| ConfigError::InvalidBackupOffsite { field, reason };
    for (field, value) in [
        ("age_recipient", &offsite.age_recipient),
        ("endpoint", &offsite.endpoint),
        ("bucket", &offsite.bucket),
    ] {
        if offsite.enabled && value.is_empty() {
            return Err(invalid(field, "required_when_enabled"));
        }
    }
    let recipient = &offsite.age_recipient;
    // age 原生 X25519 收件人编码为 62 字节；这里只检查字符集与长度，校验和由实际加密时的 age 检查。
    if !recipient.is_empty()
        && (recipient.len() != 62
            || !recipient.starts_with("age1")
            || !recipient[4..]
                .bytes()
                .all(|byte| b"qpzry9x8gf2tvdw0s3jn54khce6mua7l".contains(&byte)))
    {
        return Err(invalid("age_recipient", "invalid_age_recipient"));
    }
    if !offsite.endpoint.is_empty() && !valid_endpoint(&offsite.endpoint) {
        return Err(invalid("endpoint", "invalid_https_endpoint"));
    }
    if !offsite.bucket.is_empty() && !valid_bucket(&offsite.bucket) {
        return Err(invalid("bucket", "invalid_bucket_name"));
    }
    if !offsite.prefix.is_empty()
        && (offsite.prefix.len() > 512
            || offsite.prefix.contains("..")
            || offsite.prefix.split('/').any(|part| part.is_empty() || part == "." || part == "..")
            || !offsite
                .prefix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/-_.".contains(&byte)))
    {
        return Err(invalid("prefix", "invalid_object_prefix"));
    }
    let key = config.backup_r2_access_key_id();
    let secret = config.backup_r2_secret_access_key();
    if key.is_some() != secret.is_some() {
        return Err(invalid(
            "backup_r2_access_key_id/backup_r2_secret_access_key",
            "credentials_must_be_paired",
        ));
    }
    for (field, value) in
        [("backup_r2_access_key_id", key), ("backup_r2_secret_access_key", secret)]
    {
        if offsite.enabled && value.is_none() {
            return Err(invalid(field, "required_when_enabled"));
        }
        if value.is_some_and(|value| {
            value.is_empty()
                || value.len() > 4096
                || value.trim() != value
                || value.chars().any(char::is_control)
        }) {
            return Err(invalid(field, "invalid_credential"));
        }
    }
    Ok(())
}

fn valid_endpoint(value: &str) -> bool {
    if value.len() > 4096 || value.trim() != value || value.chars().any(char::is_control) {
        return false;
    }
    let Ok(url) = url::Url::parse(value) else { return false };
    let authority = value
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default());
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && !authority.is_some_and(|authority| authority.contains('@'))
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(url.path(), "" | "/")
}

fn valid_bucket(value: &str) -> bool {
    (3..=63).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && value.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
        && !value.starts_with("xn--")
        && !value.ends_with("-s3alias")
        && !value.ends_with("--ol-s3")
}
