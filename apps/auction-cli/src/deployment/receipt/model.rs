//! 部署回执闭集：只记录程序摘要、迁移版本、恢复点标识、操作者与时间，不含配置或秘密。
//!
//! 回执是只追加事实：同一 `receipt_id` 的文件一旦发布就不再改写；更正必须形成新回执。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 回执格式标识；读取时严格匹配，未知版本一律拒绝。
pub(crate) const FORMAT: &str = "auction-deployment-receipt.v1";
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
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub actor: String,
    pub actor_uid: u32,
    pub backup_id: Option<String>,
    pub migration: Option<MigrationRecord>,
    pub artifacts: Vec<Artifact>,
    pub failure: Option<String>,
}

impl DeploymentReceipt {
    /// 解析后必须逐字段校验；损坏或未知版本的回执不能被当成有效证据。
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if self.format != FORMAT {
            anyhow::bail!("部署回执格式不受支持：{}", self.format);
        }
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
        Ok(())
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
            if !is_sha256(sha256) || bytes == 0 {
                anyhow::bail!("部署回执的程序摘要无效");
            }
        }
        (_, None, None) => {}
        _ => anyhow::bail!("部署回执的程序摘要与字节数必须同时存在或同时缺失"),
    }
    Ok(())
}

pub(crate) fn validate_actor(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        anyhow::bail!("操作者必须是 1–128 字节、首尾无空白且不含控制字符的文本");
    }
    Ok(())
}

pub(crate) fn validate_backup_id(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
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

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::{
        Action, Artifact, DeploymentReceipt, FORMAT, MigrationRecord, Outcome, validate_actor,
        validate_backup_id,
    };

    fn receipt() -> DeploymentReceipt {
        DeploymentReceipt {
            format: FORMAT.to_owned(),
            receipt_id: uuid::Uuid::new_v4(),
            action: Action::Start,
            outcome: Outcome::Succeeded,
            started_at: chrono::Utc.with_ymd_and_hms(2026, 11, 5, 3, 0, 0).unwrap(),
            finished_at: chrono::Utc.with_ymd_and_hms(2026, 11, 5, 3, 5, 0).unwrap(),
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
        }
    }

    #[test]
    fn valid_receipt_passes_validation() {
        receipt().validate().expect("有效回执必须通过");
    }

    #[test]
    fn unknown_format_and_inconsistent_fields_are_rejected() {
        let mut value = receipt();
        value.format = "auction-deployment-receipt.v2".to_owned();
        assert!(value.validate().is_err());

        let mut value = receipt();
        value.finished_at = value.started_at - chrono::Duration::seconds(1);
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
