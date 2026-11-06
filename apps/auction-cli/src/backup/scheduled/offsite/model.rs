//! 异机副本的运行投影、待上传检查点和不可变完成事实；所有字段均不包含秘密。

use std::path::PathBuf;

use auction_integrations::BackupObject;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::{
    filesystem::FileIdentity,
    model::{ArchiveRecord, Failure, Stage},
};

pub(super) const UPLOAD_FORMAT: &str = "auction-backup-upload.v1";
pub(super) const PENDING_FORMAT: &str = "auction-backup-pending.v1";
pub(super) const COMPLETED_FORMAT: &str = "auction-backup-offsite.v1";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(in crate::backup::scheduled) struct OffsiteRecord {
    pub enabled: bool,
    pub age_version: Option<String>,
    pub daily: Option<ReplicaRecord>,
    pub monthly: Option<ReplicaRecord>,
    pub monthly_written: bool,
    pub retried_runs: Vec<Uuid>,
    pub retried_count: u64,
    pub pending_objects: Vec<BackupObject>,
    pub skipped_pending: Vec<SkippedPending>,
    pub skipped_pending_count: u64,
    pub failed_items: Vec<FailedItem>,
    pub failed_count: u64,
    pub active_run_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::backup::scheduled) struct SkippedPending {
    pub run_id: Option<Uuid>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::backup::scheduled) struct FailedItem {
    pub run_id: Uuid,
    pub stage: Stage,
    pub error_code: String,
}

impl OffsiteRecord {
    pub(super) fn skip_pending(&mut self, run_id: Option<Uuid>, reason: &'static str) {
        self.skipped_pending_count = self.skipped_pending_count.saturating_add(1);
        if self.skipped_pending.len() < 256 {
            self.skipped_pending.push(SkippedPending { run_id, reason: reason.to_owned() });
        }
    }

    pub(super) fn failed(&mut self, run_id: Uuid, failure: &Failure) {
        self.failed_count = self.failed_count.saturating_add(1);
        if self.failed_items.len() < 256 {
            self.failed_items.push(FailedItem {
                run_id,
                stage: failure.stage,
                error_code: failure.error_code.clone(),
            });
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::backup::scheduled) struct ReplicaRecord {
    pub archive: BackupObject,
    pub manifest: BackupObject,
    pub completion: BackupObject,
    pub verified_at: DateTime<Utc>,
    /// 旧 v1 副本没有范围对象；新副本在 daily/monthly 各自保存。
    #[serde(default)]
    pub scope: Option<ScopeReplica>,
}

/// 范围密文与公开 sidecar 的远端对象；sidecar 最后上传，便于下载侧先核对摘要。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::backup::scheduled) struct ScopeReplica {
    pub bundle: Option<BackupObject>,
    pub sidecar: Option<BackupObject>,
}

impl ScopeReplica {
    pub(in crate::backup::scheduled) fn complete(&self) -> Option<(BackupObject, BackupObject)> {
        Some((self.bundle.clone()?, self.sidecar.clone()?))
    }
}

/// 最后发布到远端的完成记录；生成一次后先持久化，重试不改变时间或序列化字节。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UploadRecord {
    pub format: String,
    pub run_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub archive: BackupObject,
    pub manifest: BackupObject,
    pub plaintext_manifest_sha256: String,
    pub age_version: String,
    pub verified_at: DateTime<Utc>,
    #[serde(default)]
    pub scope: Option<ScopeReplica>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplicaProgress {
    pub archive: Option<BackupObject>,
    pub manifest: Option<BackupObject>,
    pub record: Option<UploadRecord>,
    pub completion: Option<BackupObject>,
    #[serde(default)]
    pub scope: Option<ScopeReplica>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cipher {
    pub file_name: String,
    pub identity: FileIdentity,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pending {
    pub format: String,
    pub run_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub output_dir: PathBuf,
    pub database_name_sha256: String,
    pub destination_sha256: String,
    pub archive: ArchiveRecord,
    pub age_version: String,
    pub archive_cipher: Option<Cipher>,
    pub manifest_cipher: Option<Cipher>,
    /// 已封存的 archive 范围密文；不重新加密，只上传原字节。
    #[serde(default)]
    pub scope_bundle: Option<Cipher>,
    /// 范围 sidecar 是摘要投影，明文上传；它绑定密文的字节数与 SHA-256。
    #[serde(default)]
    pub scope_sidecar: Option<Cipher>,
    pub daily: ReplicaProgress,
    /// None 尚未判断；true 为本备份写月副本；false 为该月已有其他完成副本。
    pub write_monthly: Option<bool>,
    pub monthly: ReplicaProgress,
}

/// 不改写旧 failed/interrupted 回执；追加绑定同一 manifest 的副本完成事实，供保留清理核对。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CompletedRun {
    pub format: String,
    pub run_id: Uuid,
    pub output_dir: PathBuf,
    pub database_name_sha256: String,
    pub manifest_sha256: String,
    pub offsite: OffsiteRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MonthMarker {
    pub format: String,
    pub destination_sha256: String,
    pub month: String,
    pub completion: BackupObject,
}
