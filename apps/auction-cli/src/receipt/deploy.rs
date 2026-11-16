//! `deploy`/`rollback` 回执的 deploy 段。
//!
//! v2（`auction-deployment-receipt.v2`）把三个运行角色的制品身份、安装目录中 `auction-cli` 的
//! 身份与操作开始前观测到的数据库版本分开记录：rollback 只换回三个角色、保留当前 CLI，两者的
//! 来源可以不同。回退回执的身份只取自目标组件自己的事实，未知项显式为 null，不继承被回退版本的
//! 制品证据。v1 平铺段只兼容读取、不再写出；v1 rollback 回执的平铺段混有被回退版本的制品证据，
//! 不能当作已安装程序的身份。
//!
//! deploy 段描述本次操作要安装的目标组件；回执的 `artifacts` 才是实际安装或运行的程序摘要。
//! 两者只在成功回执中一致（运行摘要已核对）：安装中途失败并换回后，失败回执的 deploy 段仍是
//! 尝试安装的目标，实际程序却是换回的旧程序，因此身份只从成功回执取得（见
//! `DeploymentReceipt::proven_identity`）。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::model::{Action, MigrationRecord};
use crate::support::digest::is_lower_hex;
use crate::support::text::single_line;

/// 三个运行角色程序的制品身份；字段未知时为 null，不从其他版本推测。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RoleIdentity {
    pub artifact_directory: Option<String>,
    pub package: Option<String>,
    pub source_commit: Option<String>,
    pub source_fingerprint: Option<String>,
    pub public_payload_sha256: Option<String>,
    pub admin_payload_sha256: Option<String>,
    /// 角色程序内嵌的期望迁移版本。
    pub schema_version: Option<i64>,
}

impl RoleIdentity {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if self.artifact_directory.as_deref().is_some_and(|path| !is_absolute_path(path)) {
            anyhow::bail!("角色制品目录必须是绝对路径");
        }
        if self.package.as_deref().is_some_and(|package| !valid_package(package)) {
            anyhow::bail!("角色制品名称无效");
        }
        if self.source_commit.as_deref().is_some_and(|commit| !is_lower_hex(commit, 40))
            || [&self.source_fingerprint, &self.public_payload_sha256, &self.admin_payload_sha256]
                .into_iter()
                .flatten()
                .any(|digest| !is_lower_hex(digest, 64))
        {
            anyhow::bail!("角色制品的来源或摘要无效");
        }
        if self.schema_version.is_some_and(|version| version < 0) {
            anyhow::bail!("角色程序的 schema 版本无效");
        }
        Ok(())
    }

    /// deploy 的角色身份全部来自已校验的制品，不允许缺项。
    fn is_complete(&self) -> bool {
        self.artifact_directory.is_some()
            && self.package.is_some()
            && self.source_commit.is_some()
            && self.source_fingerprint.is_some()
            && self.public_payload_sha256.is_some()
            && self.admin_payload_sha256.is_some()
            && self.schema_version.is_some()
    }
}

/// 安装目录中 `auction-cli` 的身份；摘要总是取自实际文件，来源未知时只有摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CliIdentity {
    pub sha256: String,
    pub package: Option<String>,
    pub source_commit: Option<String>,
    /// 该 CLI 内嵌的期望迁移版本。
    pub schema_version: Option<i64>,
}

impl CliIdentity {
    /// 只知道实际文件摘要、来源未知的 CLI。
    pub(crate) fn digest_only(sha256: &str) -> Self {
        Self { sha256: sha256.to_owned(), package: None, source_commit: None, schema_version: None }
    }

    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if !is_lower_hex(&self.sha256, 64)
            || self.package.as_deref().is_some_and(|package| !valid_package(package))
            || self.source_commit.as_deref().is_some_and(|commit| !is_lower_hex(commit, 40))
            || self.schema_version.is_some_and(|version| version < 0)
        {
            anyhow::bail!("auction-cli 身份无效");
        }
        Ok(())
    }
}

/// v2 deploy 段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeployRecord {
    pub install_directory: String,
    pub roles: RoleIdentity,
    pub cli: CliIdentity,
    /// 本次操作开始前观测到的数据库迁移版本（deploy 为迁移之前）。
    pub database_version: i64,
    pub migration_drill_id: Option<String>,
    pub backup_archive: Option<String>,
    pub backup_manifest: Option<String>,
    pub rollback_record_id: Option<Uuid>,
}

impl DeployRecord {
    fn validate(&self, action: Action) -> anyhow::Result<()> {
        if !is_absolute_path(&self.install_directory) {
            anyhow::bail!("部署回执的安装目录必须是绝对路径");
        }
        self.roles.validate()?;
        self.cli.validate()?;
        if action == Action::Deploy
            && (!self.roles.is_complete()
                || self.cli.package.is_none()
                || self.cli.source_commit.is_none()
                || self.cli.schema_version.is_none())
        {
            anyhow::bail!("deploy 回执的角色与 CLI 身份必须完整");
        }
        if self.database_version < 0 {
            anyhow::bail!("部署回执观测的数据库版本无效");
        }
        validate_evidence(
            self.migration_drill_id.as_deref(),
            [&self.backup_archive, &self.backup_manifest],
            self.rollback_record_id,
        )
    }
}

/// v1 平铺段（`auction-deployment-receipt.v1`），只兼容读取，字段口径保持原样。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyDeployRecord {
    pub artifact_directory: String,
    pub package: String,
    pub source_commit: String,
    pub source_fingerprint: String,
    pub cli_sha256: String,
    pub install_directory: String,
    pub public_payload_sha256: String,
    pub admin_payload_sha256: String,
    pub migration_drill_id: Option<String>,
    pub backup_archive: Option<String>,
    pub backup_manifest: Option<String>,
    pub rollback_record_id: Option<Uuid>,
}

impl LegacyDeployRecord {
    fn validate(&self) -> anyhow::Result<()> {
        if !is_absolute_path(&self.artifact_directory) || !is_absolute_path(&self.install_directory)
        {
            anyhow::bail!("部署回执的制品与安装目录必须是绝对路径");
        }
        if !valid_package(&self.package) {
            anyhow::bail!("部署回执的制品名称无效");
        }
        if !is_lower_hex(&self.source_commit, 40)
            || !is_lower_hex(&self.source_fingerprint, 64)
            || !is_lower_hex(&self.cli_sha256, 64)
            || !is_lower_hex(&self.public_payload_sha256, 64)
            || !is_lower_hex(&self.admin_payload_sha256, 64)
        {
            anyhow::bail!("部署回执的源码或制品摘要无效");
        }
        validate_evidence(
            self.migration_drill_id.as_deref(),
            [&self.backup_archive, &self.backup_manifest],
            self.rollback_record_id,
        )
    }
}

/// 回执的 deploy 段；格式版本由回执的 `format` 决定，两者必须一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum DeploySection {
    Current(DeployRecord),
    Legacy(LegacyDeployRecord),
}

impl DeploySection {
    pub(super) fn validate(&self, action: Action) -> anyhow::Result<()> {
        match self {
            Self::Current(record) => record.validate(action),
            Self::Legacy(record) => record.validate(),
        }
    }

    /// 回执指向的回滚记录；测试据此核对回执与回滚记录共享标识。
    #[cfg(test)]
    pub(crate) fn rollback_record_id(&self) -> Option<Uuid> {
        match self {
            Self::Current(record) => record.rollback_record_id,
            Self::Legacy(record) => record.rollback_record_id,
        }
    }

    /// 本回执的操作要安装的目标组件身份；不能可靠给出时为 None。只有成功回执的目标才是实际
    /// 安装的程序，调用方经 `DeploymentReceipt::proven_identity` 取用，不直接信任失败回执。
    ///
    /// v2 原样返回。v1 deploy 段描述的正是本次安装的制品：角色与 CLI 同属一个包，内嵌 schema
    /// 等于迁移记录的期望版本（没有迁移记录时未知）。v1 rollback 段混有被回退版本的证据，
    /// 角色身份不可信，返回 None，由调用方继续向前查找或记为未知。
    pub(crate) fn target_identity(
        &self,
        action: Action,
        migration: Option<&MigrationRecord>,
    ) -> Option<(RoleIdentity, CliIdentity)> {
        match self {
            Self::Current(record) => Some((record.roles.clone(), record.cli.clone())),
            Self::Legacy(record) if action == Action::Deploy => {
                let schema_version = migration.map(|migration| migration.expected_version);
                Some((
                    RoleIdentity {
                        artifact_directory: Some(record.artifact_directory.clone()),
                        package: Some(record.package.clone()),
                        source_commit: Some(record.source_commit.clone()),
                        source_fingerprint: Some(record.source_fingerprint.clone()),
                        public_payload_sha256: Some(record.public_payload_sha256.clone()),
                        admin_payload_sha256: Some(record.admin_payload_sha256.clone()),
                        schema_version,
                    },
                    CliIdentity {
                        sha256: record.cli_sha256.clone(),
                        package: Some(record.package.clone()),
                        source_commit: Some(record.source_commit.clone()),
                        schema_version,
                    },
                ))
            }
            Self::Legacy(_) => None,
        }
    }
}

fn validate_evidence(
    migration_drill_id: Option<&str>,
    backups: [&Option<String>; 2],
    rollback_record_id: Option<Uuid>,
) -> anyhow::Result<()> {
    if migration_drill_id.is_some_and(|drill| !single_line(drill, 128)) {
        anyhow::bail!("部署回执的迁移演练标识无效");
    }
    if backups.into_iter().flatten().any(|path| !is_absolute_path(path)) {
        anyhow::bail!("部署回执的备份路径必须是绝对路径");
    }
    if rollback_record_id.is_some_and(|id| id.is_nil()) {
        anyhow::bail!("部署回执的回滚记录标识不能是 nil UUID");
    }
    Ok(())
}

fn valid_package(package: &str) -> bool {
    !package.is_empty()
        && package.len() <= 256
        && !package.contains('/')
        && !package.chars().any(char::is_control)
}

pub(super) fn is_absolute_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 4096
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        Action, CliIdentity, DeployRecord, DeploySection, LegacyDeployRecord, MigrationRecord,
        RoleIdentity,
    };

    fn legacy() -> LegacyDeployRecord {
        LegacyDeployRecord {
            artifact_directory: "/srv/auction-release".to_owned(),
            package: "auction-rust-0.1.0-aaaaaaaaaaaa".to_owned(),
            source_commit: "a".repeat(40),
            source_fingerprint: "b".repeat(64),
            cli_sha256: "c".repeat(64),
            install_directory: "/opt/auction/bin".to_owned(),
            public_payload_sha256: "d".repeat(64),
            admin_payload_sha256: "e".repeat(64),
            migration_drill_id: None,
            backup_archive: None,
            backup_manifest: None,
            rollback_record_id: Some(Uuid::new_v4()),
        }
    }

    fn rollback_record() -> DeployRecord {
        DeployRecord {
            install_directory: "/opt/auction/bin".to_owned(),
            roles: RoleIdentity { schema_version: Some(96), ..RoleIdentity::default() },
            cli: CliIdentity::digest_only(&"f".repeat(64)),
            database_version: 98,
            migration_drill_id: None,
            backup_archive: None,
            backup_manifest: None,
            rollback_record_id: Some(Uuid::new_v4()),
        }
    }

    /// v1 与 v2 deploy 段按字段形状区分；JSON 往返后保持原版本，不互相冒充。
    #[test]
    fn sections_round_trip_in_their_own_shape() {
        for section in [DeploySection::Legacy(legacy()), DeploySection::Current(rollback_record())]
        {
            let json = serde_json::to_value(&section).expect("序列化");
            let parsed: DeploySection = serde_json::from_value(json).expect("反序列化");
            assert_eq!(parsed, section);
        }
        let mut mixed = serde_json::to_value(DeploySection::Legacy(legacy())).expect("序列化");
        mixed["database_version"] = serde_json::json!(1);
        assert!(serde_json::from_value::<DeploySection>(mixed).is_err(), "混合字段必须拒绝");
    }

    /// 回退回执允许未知身份（null），deploy 回执的身份必须完整。
    #[test]
    fn rollback_identity_may_be_unknown_but_deploy_must_be_complete() {
        let section = DeploySection::Current(rollback_record());
        section.validate(Action::Rollback).expect("回退回执允许未知身份");
        assert!(section.validate(Action::Deploy).is_err());

        let mut invalid = rollback_record();
        invalid.roles.source_commit = Some("A".repeat(40));
        assert!(DeploySection::Current(invalid).validate(Action::Rollback).is_err());
        let mut invalid = rollback_record();
        invalid.cli.sha256 = "c".repeat(63);
        assert!(DeploySection::Current(invalid).validate(Action::Rollback).is_err());
        let mut invalid = rollback_record();
        invalid.database_version = -1;
        assert!(DeploySection::Current(invalid).validate(Action::Rollback).is_err());
    }

    /// v1 deploy 段给出同包的角色与 CLI 身份，schema 取迁移记录的期望版本；v1 rollback 段的
    /// 角色证据混有被回退版本，不能作为已安装身份。
    #[test]
    fn legacy_identity_is_trusted_only_for_deploy_receipts() {
        let section = DeploySection::Legacy(legacy());
        let migration = MigrationRecord {
            before_version: 94,
            after_version: 96,
            expected_version: 96,
            applied: 2,
        };
        let (roles, cli) = section
            .target_identity(Action::Deploy, Some(&migration))
            .expect("v1 deploy 段可作为身份");
        assert_eq!(roles.package.as_deref(), Some("auction-rust-0.1.0-aaaaaaaaaaaa"));
        assert_eq!(roles.schema_version, Some(96));
        assert_eq!(cli.sha256, "c".repeat(64));
        assert_eq!(cli.schema_version, Some(96));
        let (roles, _) = section.target_identity(Action::Deploy, None).expect("缺迁移记录");
        assert_eq!(roles.schema_version, None, "没有迁移记录时 schema 未知");
        assert!(section.target_identity(Action::Rollback, Some(&migration)).is_none());
    }
}
