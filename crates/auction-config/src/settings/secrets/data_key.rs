//! 数据密钥环的配置投影：当前写入密钥、保留解密的旧密钥与 12 个月保留期。
//!
//! 敏感 TOML 中每个域提供 `*_data_key_id`/`*_data_key` 作为当前密钥，另以
//! `*_previous_data_keys` 数组列出旧密钥。旧条目必须给出 `retired_at`（停止用于新写入的
//! UTC 时间，RFC 3339）；保留期不短于最长备份保留期（12 个月），因此 `retain_until` 由
//! 配置计算而不是由运维填写，避免把保留期配短。

use chrono::{DateTime, Months, Utc};
use serde::Deserialize;

use auction_domain::DataKeyDomain;

use super::super::ConfigError;
use super::model::SecretString;

/// 旧密钥最短保留月数；与 最长备份保留期一致。
pub const DATA_KEY_RETENTION_MONTHS: u32 = 12;

/// 旧密钥条目的敏感 TOML 形状；未知字段失败关闭。
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviousDataKeyConfig {
    pub(crate) id: String,
    pub(crate) key: String,
    pub(crate) retired_at: String,
}

/// 已校验的旧数据密钥；材料只在装配根读取，`Debug` 永远脱敏。
#[derive(Clone)]
pub struct RetiredDataKey {
    pub(crate) id: String,
    pub(crate) material: SecretString,
    pub(crate) retired_at: DateTime<Utc>,
    pub(crate) retain_until: DateTime<Utc>,
}

impl RetiredDataKey {
    /// 数据密钥标识；非秘密，可写入日志与回执。
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 密钥材料；调用方不得输出、持久化或复制到日志/DTO。
    pub fn material(&self) -> &str {
        self.material.expose_secret()
    }

    /// 停止用于新写入的时间。
    pub fn retired_at(&self) -> DateTime<Utc> {
        self.retired_at
    }

    /// 最早允许从配置移除的时间；保留期不短于 12 个月。
    pub fn retain_until(&self) -> DateTime<Utc> {
        self.retain_until
    }
}

impl std::fmt::Debug for RetiredDataKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RetiredDataKey")
            .field("id", &self.id)
            .field("retired_at", &self.retired_at)
            .field("retain_until", &self.retain_until)
            .finish()
    }
}

/// 校验并转换单个域的旧密钥数组。
pub(crate) fn retired_keys(
    domain: DataKeyDomain,
    current_id: Option<&str>,
    entries: Vec<PreviousDataKeyConfig>,
) -> Result<Vec<RetiredDataKey>, ConfigError> {
    let mut keys = Vec::with_capacity(entries.len());
    let now = Utc::now();
    for entry in entries {
        if !auction_domain::valid_key_id(&entry.id) {
            return Err(invalid_retired_key(domain, "id 必须是 1 至 64 字节的密钥标识"));
        }
        if current_id == Some(entry.id.as_str()) {
            return Err(invalid_retired_key(domain, "旧密钥标识不能等于当前密钥标识"));
        }
        if keys.iter().any(|key: &RetiredDataKey| key.id == entry.id) {
            return Err(invalid_retired_key(domain, "旧密钥标识重复"));
        }
        let retired_at = DateTime::parse_from_rfc3339(&entry.retired_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|_| invalid_retired_key(domain, "retired_at 必须是 RFC 3339 时间"))?;
        if retired_at > now {
            return Err(invalid_retired_key(domain, "retired_at 不能在未来"));
        }
        let retain_until = retired_at
            .checked_add_months(Months::new(DATA_KEY_RETENTION_MONTHS))
            .ok_or_else(|| invalid_retired_key(domain, "retired_at + 12 个月溢出"))?;
        let material = SecretString::new(retired_material_field(domain), entry.key)?;
        keys.push(RetiredDataKey { id: entry.id, material, retired_at, retain_until });
    }
    Ok(keys)
}

const fn retired_material_field(domain: DataKeyDomain) -> &'static str {
    match domain {
        DataKeyDomain::Equipment => "equipment_previous_data_keys.key",
        DataKeyDomain::UserLogin => "user_login_previous_data_keys.key",
        DataKeyDomain::Credential => "credential_previous_data_keys.key",
        DataKeyDomain::MailDiagnostics => "mail_diagnostics_previous_data_keys.key",
    }
}

fn invalid_retired_key(domain: DataKeyDomain, reason: &str) -> ConfigError {
    ConfigError::InvalidDataKeyRing { domain: domain.as_str(), reason: reason.to_owned() }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Months, Utc};

    use super::*;

    fn entry(id: &str, retired_at: chrono::DateTime<Utc>) -> PreviousDataKeyConfig {
        PreviousDataKeyConfig {
            id: id.to_owned(),
            key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned(),
            retired_at: retired_at.to_rfc3339(),
        }
    }

    #[test]
    fn retain_until_is_exactly_twelve_months() {
        let retired_at = Utc::now() - Duration::days(30);
        let keys = retired_keys(
            DataKeyDomain::Equipment,
            Some("equipment-2026-11"),
            vec![entry("equipment-2025-11", retired_at)],
        )
        .expect("旧密钥条目有效");
        assert_eq!(keys.len(), 1);
        let key = &keys[0];
        assert_eq!(key.id(), "equipment-2025-11");
        assert_eq!(key.retired_at(), retired_at);
        assert_eq!(
            key.retain_until(),
            retired_at.checked_add_months(Months::new(DATA_KEY_RETENTION_MONTHS)).unwrap()
        );
        assert!(!format!("{key:?}").contains("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="));
    }

    #[test]
    fn future_or_malformed_retirement_is_rejected() {
        let future = Utc::now() + Duration::days(1);
        assert!(matches!(
            retired_keys(
                DataKeyDomain::Credential,
                Some("credential-2026-11"),
                vec![entry("credential-2025-11", future)],
            ),
            Err(ConfigError::InvalidDataKeyRing { .. })
        ));
        let malformed = PreviousDataKeyConfig {
            id: "credential-2025-11".to_owned(),
            key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned(),
            retired_at: "2025-11-02".to_owned(),
        };
        assert!(matches!(
            retired_keys(DataKeyDomain::Credential, Some("credential-2026-11"), vec![malformed]),
            Err(ConfigError::InvalidDataKeyRing { .. })
        ));
    }

    #[test]
    fn duplicate_or_current_key_is_rejected() {
        let retired_at = Utc::now() - Duration::days(1);
        assert!(matches!(
            retired_keys(
                DataKeyDomain::UserLogin,
                Some("user_login-2026-11"),
                vec![entry("user_login-2026-11", retired_at)],
            ),
            Err(ConfigError::InvalidDataKeyRing { .. })
        ));
        assert!(matches!(
            retired_keys(
                DataKeyDomain::UserLogin,
                Some("user_login-2026-11"),
                vec![
                    entry("user_login-2025-11", retired_at),
                    entry("user_login-2025-11", retired_at),
                ],
            ),
            Err(ConfigError::InvalidDataKeyRing { .. })
        ));
        assert!(matches!(
            retired_keys(
                DataKeyDomain::UserLogin,
                Some("user_login-2026-11"),
                vec![entry("bad key id", retired_at)],
            ),
            Err(ConfigError::InvalidDataKeyRing { .. })
        ));
    }
}
