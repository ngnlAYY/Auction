//! 数据库之外的恢复范围：运行配置、加密邮件诊断证据与部署回执。
//!
//! 这些材料都可能含秘密或玩家信息，因此只描述来源与上限；内容一律经 age 加密后
//! 写入备份目录，公开 manifest 只记录摘要，不记录绝对路径或文件内容。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::ConfigError;

/// 默认总字节上限 256 MiB；邮件诊断归档本身受 Worker 的 1 GiB 上限约束。
pub const DEFAULT_MAXIMUM_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
pub const MIN_MAXIMUM_TOTAL_BYTES: u64 = 1024 * 1024;
pub const MAX_MAXIMUM_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
pub const DEFAULT_MAX_FILES: u32 = 4096;
pub const MAX_MAX_FILES: u32 = 8192;

/// 备份范围策略；只由 CLI 消费，常驻角色不据此启动任何任务。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackupScopeConfig {
    /// 默认关闭；启用后下一次备份即纳入范围，无需重启角色。
    pub enabled: bool,
    /// 纳入本轮加载到的运行配置文件（角色、common、敏感与可选 runtime）。
    pub runtime_config: bool,
    /// 纳入 Worker 邮件诊断归档目录中的证据文件。
    pub mail_diagnostics: bool,
    /// 可选部署回执目录；为空表示不纳入。必须是绝对路径，不递归越出该目录。
    pub deployment_receipts_dir: PathBuf,
    /// 文件数上限，范围 1–8192。
    pub max_files: u32,
    /// 全部证据明文字节上限，范围 1 MiB–1 GiB。
    pub maximum_total_bytes: u64,
}

impl Default for BackupScopeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            runtime_config: true,
            mail_diagnostics: true,
            deployment_receipts_dir: PathBuf::new(),
            max_files: DEFAULT_MAX_FILES,
            maximum_total_bytes: DEFAULT_MAXIMUM_TOTAL_BYTES,
        }
    }
}

/// 冻结校验结果；只包含结构约束，不读取任何来源目录。
pub(super) fn validate(scope: &BackupScopeConfig, age_recipient: &str) -> Result<(), ConfigError> {
    if !(1..=MAX_MAX_FILES).contains(&scope.max_files) {
        return Err(ConfigError::InvalidBackupScope("max_files 必须为 1–8192"));
    }
    if !(MIN_MAXIMUM_TOTAL_BYTES..=MAX_MAXIMUM_TOTAL_BYTES).contains(&scope.maximum_total_bytes) {
        return Err(ConfigError::InvalidBackupScope("maximum_total_bytes 必须为 1 MiB–1 GiB"));
    }
    if !scope.deployment_receipts_dir.as_os_str().is_empty() {
        let path = &scope.deployment_receipts_dir;
        if !path.is_absolute()
            || path.components().any(|part| matches!(part, std::path::Component::ParentDir))
            || path.to_string_lossy().chars().any(char::is_control)
        {
            return Err(ConfigError::InvalidBackupScope(
                "deployment_receipts_dir 必须是绝对路径且不含 `..` 或控制字符",
            ));
        }
    }
    if scope.enabled && age_recipient.is_empty() {
        return Err(ConfigError::InvalidBackupScope(
            "启用备份范围时必须配置 [backup.offsite].age_recipient；范围内容一律加密",
        ));
    }
    if scope.enabled
        && !scope.runtime_config
        && !scope.mail_diagnostics
        && scope.deployment_receipts_dir.as_os_str().is_empty()
    {
        return Err(ConfigError::InvalidBackupScope("启用备份范围时至少要选择一个来源"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{BackupScopeConfig, validate};

    fn enabled() -> BackupScopeConfig {
        BackupScopeConfig { enabled: true, ..BackupScopeConfig::default() }
    }

    #[test]
    fn disabled_scope_accepts_defaults_without_age_recipient() {
        assert!(validate(&BackupScopeConfig::default(), "").is_ok());
    }

    #[test]
    fn enabled_scope_requires_encryption_recipient() {
        assert!(validate(&enabled(), "").is_err());
        assert!(
            validate(&enabled(), "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg2g2g2g2g2g2g2g")
                .is_ok()
        );
    }

    #[test]
    fn enabled_scope_requires_at_least_one_source() {
        let scope =
            BackupScopeConfig { runtime_config: false, mail_diagnostics: false, ..enabled() };
        assert!(validate(&scope, "age1x").is_err());
        let scope = BackupScopeConfig {
            deployment_receipts_dir: PathBuf::from("/var/lib/auction/receipts"),
            ..scope
        };
        assert!(validate(&scope, "age1x").is_ok());
    }

    #[test]
    fn scope_limits_and_receipts_path_are_bounded() {
        for (field, scope) in [
            ("max_files", BackupScopeConfig { max_files: 0, ..enabled() }),
            ("max_files", BackupScopeConfig { max_files: 8193, ..enabled() }),
            ("maximum_total_bytes", BackupScopeConfig { maximum_total_bytes: 1024, ..enabled() }),
            (
                "deployment_receipts_dir",
                BackupScopeConfig {
                    deployment_receipts_dir: PathBuf::from("relative/dir"),
                    ..enabled()
                },
            ),
            (
                "deployment_receipts_dir",
                BackupScopeConfig {
                    deployment_receipts_dir: PathBuf::from("/var/lib/../etc/auction"),
                    ..enabled()
                },
            ),
        ] {
            assert!(validate(&scope, "age1x").is_err(), "{field} 应被拒绝");
        }
    }
}
