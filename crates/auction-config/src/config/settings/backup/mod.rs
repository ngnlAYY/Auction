//! CLI 专用的定时备份策略；常驻角色只保留配置，不据此启动任务或校验运维阈值。

use serde::{Deserialize, Serialize};

use super::{AppConfig, ConfigError};

mod offsite;
mod scope;

pub use offsite::BackupOffsiteConfig;
pub use scope::BackupScopeConfig;

/// 定时逻辑备份的本地保留、水位与总预算；目录和库名由 CLI 参数指定。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackupConfig {
    /// 本地保留最近多少份已完成备份，范围 1–365。
    pub local_retention_count: u16,
    /// 备份所在文件系统必须留出的可用字节，范围 64 MiB–1 TiB。
    pub min_free_bytes: u64,
    /// 文件系统必须留出的可用空间百分比，范围 1–50。
    pub min_free_percent: u8,
    /// 从本次启动开始计算的任务总预算，范围 60–7200 秒。
    pub timeout_seconds: u64,
    /// 可选异机副本；仅 CLI 消费，不在常驻角色中启动上传。
    pub offsite: BackupOffsiteConfig,
    /// 数据库之外的恢复范围；默认关闭，内容一律 age 加密后保存。
    pub scope: BackupScopeConfig,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            local_retention_count: 7,
            min_free_bytes: 1024 * 1024 * 1024,
            min_free_percent: 10,
            timeout_seconds: 7200,
            offsite: BackupOffsiteConfig::default(),
            scope: BackupScopeConfig::default(),
        }
    }
}

impl AppConfig {
    /// 只供 CLI 的配置检查和备份入口调用；不改变三角色的运行参数校验。
    pub fn validate_backup(&self) -> Result<(), ConfigError> {
        let backup = &self.backup;
        if !(1..=365).contains(&backup.local_retention_count) {
            return Err(ConfigError::InvalidBackupConfig("local_retention_count 必须为 1–365"));
        }
        if !(64 * 1024 * 1024..=1024 * 1024 * 1024 * 1024).contains(&backup.min_free_bytes) {
            return Err(ConfigError::InvalidBackupConfig("min_free_bytes 必须为 64 MiB–1 TiB"));
        }
        if !(1..=50).contains(&backup.min_free_percent) {
            return Err(ConfigError::InvalidBackupConfig("min_free_percent 必须为 1–50"));
        }
        if !(60..=7200).contains(&backup.timeout_seconds) {
            return Err(ConfigError::InvalidBackupConfig("timeout_seconds 必须为 60–7200"));
        }
        offsite::validate(self)?;
        scope::validate(&backup.scope, &backup.offsite.age_recipient)?;
        for (field, value) in [
            ("backup_heartbeat_success_url", self.backup_heartbeat_success_url()),
            ("backup_heartbeat_failure_url", self.backup_heartbeat_failure_url()),
        ] {
            if let Some(value) = value {
                validate_heartbeat_url(field, value)?;
            }
        }
        Ok(())
    }
}

fn validate_heartbeat_url(field: &'static str, value: &str) -> Result<(), ConfigError> {
    let invalid = || ConfigError::InvalidBackupHeartbeatUrl { field };
    if value.len() > 4096 || value.trim() != value || value.chars().any(char::is_control) {
        return Err(invalid());
    }
    let url = url::Url::parse(value).map_err(|_| invalid())?;
    // 空 userinfo 也拒绝，防止解析器规范化后隐藏原输入中的认证段。
    let authority = value
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default());
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || authority.is_some_and(|authority| authority.contains('@'))
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(())
}
