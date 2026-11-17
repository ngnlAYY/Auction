//! 部署回执闭集：只记录程序摘要、迁移版本、恢复点标识、操作者与时间，不含配置或秘密。
//!
//! 回执是只追加事实：同一 `receipt_id` 的文件一旦发布就不再改写；更正必须形成新回执。新回执
//! 写 v2；v1 回执原样兼容读取，deploy 段版本必须与回执格式一致。

use auction_time::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::deploy::{CliIdentity, DeploySection, RoleIdentity};
use crate::support::digest::is_lower_hex;
use crate::support::text::single_line;

/// 新回执的格式标识；读取时只接受它与 [`LEGACY_FORMAT`]，未知版本一律拒绝。
pub(crate) const FORMAT: &str = "auction-deployment-receipt.v2";
/// 只读兼容的旧格式：deploy 段为平铺字段，见 [`super::deploy::LegacyDeployRecord`]。
pub(crate) const LEGACY_FORMAT: &str = "auction-deployment-receipt.v1";
/// 失败摘要上限；只保留固定长度文本，避免把完整错误链写入长期证据。
pub(crate) const MAX_FAILURE_CHARS: usize = 1024;
/// 单条回执字节上限。
pub(crate) const MAX_RECEIPT_BYTES: u64 = 1024 * 1024;
/// 一次分体部署固定记录三个角色的程序摘要。
const MAX_ARTIFACTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    Start,
    Restart,
    Deploy,
    Rollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Succeeded,
    AlreadyRunning,
    Failed,
}

/// 本次启动实际执行的迁移结果；`already_running` 等未迁移的情况为 `None`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MigrationRecord {
    pub before_version: i64,
    pub after_version: i64,
    pub expected_version: i64,
    pub applied: i64,
}

/// 实际运行的三角色程序身份；摘要取自 `/proc/<pid>/exe`，不信任安装路径。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Artifact {
    pub role: String,
    pub path: Option<String>,
    pub sha256: Option<String>,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeploymentReceipt {
    pub format: String,
    pub receipt_id: Uuid,
    pub action: Action,
    pub outcome: Outcome,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub actor: String,
    pub actor_uid: u32,
    pub backup_id: Option<String>,
    pub migration: Option<MigrationRecord>,
    pub artifacts: Vec<Artifact>,
    pub failure: Option<String>,
    /// `service start/restart` 回执没有本段；deploy/rollback 成功时必填。
    #[serde(default)]
    pub deploy: Option<DeploySection>,
}

impl DeploymentReceipt {
    /// 解析后必须逐字段校验；损坏或未知版本的回执不能被当成有效证据。
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        let legacy = match self.format.as_str() {
            FORMAT => false,
            LEGACY_FORMAT => true,
            other => anyhow::bail!("部署回执格式不受支持：{other}"),
        };
        if self.receipt_id.is_nil() {
            anyhow::bail!("部署回执缺少有效的 receipt_id");
        }
        if self.finished_at < self.started_at {
            anyhow::bail!("部署回执的完成时间早于开始时间");
        }
        validate_actor(&self.actor)?;
        if let Some(backup_id) = &self.backup_id {
            validate_backup_id(backup_id)?;
        }
        if let Some(migration) = &self.migration
            && (migration.before_version < 0
                || migration.after_version < migration.before_version
                || migration.expected_version < 0
                || migration.applied != migration.after_version - migration.before_version)
        {
            anyhow::bail!("部署回执的迁移版本自相矛盾");
        }
        match (self.outcome, &self.failure) {
            (Outcome::Failed, Some(failure)) => validate_failure(failure)?,
            (Outcome::Failed, None) => anyhow::bail!("失败的部署回执必须记录失败摘要"),
            (_, Some(_)) => anyhow::bail!("成功的部署回执不能记录失败摘要"),
            (_, None) => {}
        }
        if self.artifacts.len() > MAX_ARTIFACTS {
            anyhow::bail!("部署回执的程序摘要超出上限");
        }
        let mut roles = Vec::with_capacity(self.artifacts.len());
        for artifact in &self.artifacts {
            validate_artifact(artifact)?;
            if roles.contains(&artifact.role.as_str()) {
                anyhow::bail!("部署回执重复记录角色 {}", artifact.role);
            }
            roles.push(artifact.role.as_str());
        }
        match (self.action, self.outcome, &self.deploy) {
            (Action::Deploy | Action::Rollback, _, Some(section)) => {
                if legacy != matches!(section, DeploySection::Legacy(_)) {
                    anyhow::bail!("部署回执的 deploy 段版本与回执格式不一致");
                }
                section.validate(self.action)?;
            }
            (Action::Deploy | Action::Rollback, Outcome::Failed, None) => {}
            (Action::Deploy | Action::Rollback, _, None) => {
                anyhow::bail!("deploy/rollback 成功回执必须记录制品与回滚证据");
            }
            (Action::Start | Action::Restart, _, Some(_)) => {
                anyhow::bail!("service 回执不能记录 deploy 段");
            }
            (Action::Start | Action::Restart, _, None) => {}
        }
        Ok(())
    }

    /// 本回执能证明的已安装组件身份。只有成功回执的部署目标经运行程序摘要核对等于实际安装的
    /// 组件；失败回执的 deploy 段只是尝试安装的目标（安装中途失败并换回后实际仍是旧程序），
    /// 不能作为任何组件的身份来源，返回 None。
    pub(crate) fn proven_identity(&self) -> Option<(RoleIdentity, CliIdentity)> {
        if self.outcome != Outcome::Succeeded {
            return None;
        }
        self.deploy.as_ref()?.target_identity(self.action, self.migration.as_ref())
    }
}

impl Artifact {
    pub(crate) fn role(role: &str) -> anyhow::Result<Self> {
        if !matches!(role, "worker" | "admin" | "public") {
            anyhow::bail!("部署回执的角色必须是 worker、admin 或 public");
        }
        Ok(Self { role: role.to_owned(), path: None, sha256: None, bytes: None })
    }
}

fn validate_artifact(artifact: &Artifact) -> anyhow::Result<()> {
    if !matches!(artifact.role.as_str(), "worker" | "admin" | "public") {
        anyhow::bail!("部署回执的角色必须是 worker、admin 或 public");
    }
    if let Some(path) = &artifact.path
        && (path.is_empty()
            || path.len() > 4096
            || !path.starts_with('/')
            || path.chars().any(char::is_control))
    {
        anyhow::bail!("部署回执的程序路径必须是绝对路径且不含控制字符");
    }
    match (artifact.path.is_some(), artifact.sha256.as_deref(), artifact.bytes) {
        (_, Some(sha256), Some(bytes)) => {
            if !is_lower_hex(sha256, 64) || bytes == 0 {
                anyhow::bail!("部署回执的程序摘要无效");
            }
        }
        (_, None, None) => {}
        _ => anyhow::bail!("部署回执的程序摘要与字节数必须同时存在或同时缺失"),
    }
    Ok(())
}

pub(crate) fn validate_actor(value: &str) -> anyhow::Result<()> {
    if !single_line(value, 128) {
        anyhow::bail!("操作者必须是 1–128 字节、首尾无空白且不含控制字符的文本");
    }
    Ok(())
}

pub(crate) fn validate_backup_id(value: &str) -> anyhow::Result<()> {
    if !single_line(value, 256) {
        anyhow::bail!("--backup-id 必须是 1–256 字节、首尾无空白且不含控制字符的文本");
    }
    Ok(())
}

fn validate_failure(value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty()
        || value.chars().count() > MAX_FAILURE_CHARS
        || value.chars().any(char::is_control)
    {
        anyhow::bail!("部署回执的失败摘要无效");
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    use super::{
        Action, Artifact, DeploymentReceipt, FORMAT, LEGACY_FORMAT, MigrationRecord, Outcome,
        validate_actor, validate_backup_id,
    };
    use crate::receipt::deploy::{
        CliIdentity, DeployRecord, DeploySection, LegacyDeployRecord, RoleIdentity,
    };

    fn receipt() -> DeploymentReceipt {
        DeploymentReceipt {
            format: FORMAT.to_owned(),
            receipt_id: uuid::Uuid::new_v4(),
            action: Action::Start,
            outcome: Outcome::Succeeded,
            started_at: auction_time::Timestamp::from_ymd_hms(2026, 11, 5, 3, 0, 0).unwrap(),
            finished_at: auction_time::Timestamp::from_ymd_hms(2026, 11, 5, 3, 5, 0).unwrap(),
            actor: "root".to_owned(),
            actor_uid: 0,
            backup_id: Some("backup-20261105".to_owned()),
            migration: Some(MigrationRecord {
                before_version: 95,
                after_version: 95,
                expected_version: 95,
                applied: 0,
            }),
            artifacts: vec![Artifact {
                role: "worker".to_owned(),
                path: Some("/opt/auction/bin/auction-worker".to_owned()),
                sha256: Some("a".repeat(64)),
                bytes: Some(1024),
            }],
            failure: None,
            deploy: None,
        }
    }

    #[test]
    fn valid_receipt_passes_validation() {
        receipt().validate().expect("有效回执必须通过");
    }

    #[test]
    fn unknown_format_and_inconsistent_fields_are_rejected() {
        let mut value = receipt();
        value.format = "auction-deployment-receipt.v3".to_owned();
        assert!(value.validate().is_err());
        let mut value = receipt();
        value.format = LEGACY_FORMAT.to_owned();
        value.validate().expect("v1 service 回执仍可读取");

        let mut value = receipt();
        value.finished_at = value.started_at - auction_time::Duration::seconds(1);
        assert!(value.validate().is_err());

        let mut value = receipt();
        value.migration = Some(MigrationRecord {
            before_version: 95,
            after_version: 94,
            expected_version: 95,
            applied: 0,
        });
        assert!(value.validate().is_err());

        let mut value = receipt();
        value.outcome = Outcome::Failed;
        assert!(value.validate().is_err());

        let mut value = receipt();
        value.outcome = Outcome::Failed;
        value.failure = Some("systemctl start 失败".to_owned());
        value.validate().expect("失败回执必须有摘要");
    }

    #[test]
    fn artifact_hash_must_be_lowercase_hexadecimal() {
        let mut value = receipt();
        value.artifacts[0].sha256 = Some("A".repeat(64));
        assert!(value.validate().is_err());
        value.artifacts[0].sha256 = Some("a".repeat(63));
        assert!(value.validate().is_err());
        value.artifacts[0].sha256 = Some("a".repeat(64));
        value.artifacts[0].bytes = None;
        assert!(value.validate().is_err());
        value.artifacts[0].bytes = Some(1024);
        value.validate().expect("摘要与字节数同时存在");
    }

    #[test]
    fn duplicate_artifact_roles_are_rejected() {
        let mut value = receipt();
        value.artifacts.push(value.artifacts[0].clone());
        assert!(value.validate().is_err());
    }

    fn deploy_record() -> DeployRecord {
        let package = Some("auction-rust-0.1.0-aaaaaaaaaaaa".to_owned());
        let source_commit = Some("a".repeat(40));
        DeployRecord {
            install_directory: "/opt/auction/bin".to_owned(),
            roles: RoleIdentity {
                artifact_directory: Some("/srv/auction-release".to_owned()),
                package: package.clone(),
                source_commit: source_commit.clone(),
                source_fingerprint: Some("b".repeat(64)),
                public_payload_sha256: Some("d".repeat(64)),
                admin_payload_sha256: Some("e".repeat(64)),
                schema_version: Some(96),
            },
            cli: CliIdentity {
                sha256: "c".repeat(64),
                package,
                source_commit,
                schema_version: Some(96),
            },
            database_version: 95,
            migration_drill_id: Some("drill-20261105".to_owned()),
            backup_archive: Some("/var/backups/auction/a.dump".to_owned()),
            backup_manifest: Some("/var/backups/auction/a.dump.manifest".to_owned()),
            rollback_record_id: Some(uuid::Uuid::new_v4()),
        }
    }

    fn legacy_record() -> LegacyDeployRecord {
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
            rollback_record_id: Some(uuid::Uuid::new_v4()),
        }
    }

    #[test]
    fn deploy_receipts_record_artifact_and_rollback_evidence() {
        let mut value = receipt();
        value.action = Action::Deploy;
        assert!(value.validate().is_err(), "成功 deploy 必须记录 deploy 段");

        value.deploy = Some(DeploySection::Current(deploy_record()));
        value.validate().expect("deploy 回执必须通过");

        let mut value = receipt();
        value.deploy = Some(DeploySection::Current(deploy_record()));
        assert!(value.validate().is_err(), "service 回执不能记录 deploy 段");

        let mut value = receipt();
        value.action = Action::Rollback;
        value.outcome = Outcome::Failed;
        value.deploy = Some(DeploySection::Current(deploy_record()));
        value.failure = Some("启动失败".to_owned());
        value.validate().expect("失败回退允许保留已知制品证据");

        let mut value = receipt();
        value.action = Action::Deploy;
        let mut record = deploy_record();
        record.cli.sha256 = "C".repeat(64);
        value.deploy = Some(DeploySection::Current(record));
        assert!(value.validate().is_err());
    }

    /// deploy 段版本必须与回执格式一致：v1 回执只能带平铺段，v2 回执只能带分段身份。
    #[test]
    fn deploy_section_version_follows_the_receipt_format() {
        let mut value = receipt();
        value.action = Action::Deploy;
        value.format = LEGACY_FORMAT.to_owned();
        value.deploy = Some(DeploySection::Legacy(legacy_record()));
        value.validate().expect("v1 deploy 回执原样可读");
        let json = serde_json::to_vec(&value).expect("序列化");
        let parsed: DeploymentReceipt = serde_json::from_slice(&json).expect("v1 回执可解析");
        assert_eq!(parsed, value);

        value.format = FORMAT.to_owned();
        assert!(value.validate().is_err(), "v2 回执不能带 v1 平铺段");
        value.format = LEGACY_FORMAT.to_owned();
        value.deploy = Some(DeploySection::Current(deploy_record()));
        assert!(value.validate().is_err(), "v1 回执不能带 v2 分段身份");
    }

    #[test]
    fn actor_and_backup_id_are_bounded() {
        assert!(validate_actor("root").is_ok());
        assert!(validate_actor("").is_err());
        assert!(validate_actor(" root").is_err());
        assert!(validate_actor("ro\n").is_err());
        assert!(validate_actor(&"a".repeat(129)).is_err());
        assert!(validate_backup_id("backup-20261105T030000Z").is_ok());
        assert!(validate_backup_id("").is_err());
        assert!(validate_backup_id("id\u{7}").is_err());
    }
}
