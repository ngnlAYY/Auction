//! 上传报告、对象与记录模型。

use std::path::PathBuf;

use auction_integrations::{BackupObject, BackupObjectStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub(super) struct Report {
    pub(super) outcome: UploadOutcome,
    pub(super) staging_dir: PathBuf,
    pub(super) uploaded: Vec<Uploaded>,
    pub(super) already_present: Vec<Uploaded>,
    pub(super) failed: Vec<Failed>,
    pub(super) invalid: Vec<Failed>,
    pub(super) incomplete: Vec<String>,
    pub(super) remaining: usize,
    /// 每周基础备份的独立结果；保留与删除只作用于本地暂存目录。
    pub(super) base: BaseReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum UploadOutcome {
    Completed,
    Failed,
    Skipped,
}

impl UploadOutcome {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct Uploaded {
    pub(super) segment_name: String,
    pub(super) key: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Failed {
    /// WAL 段名或基础备份 ID。
    pub(super) name: String,
    pub(super) reason: String,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct BaseReport {
    pub(super) uploaded: Vec<BaseUploaded>,
    pub(super) already_present: Vec<BaseUploaded>,
    /// 已有本机上传证据、本次未重复回读的备份 ID。
    pub(super) already_verified: Vec<String>,
    pub(super) failed: Vec<Failed>,
    /// 当前保留在暂存目录的基础备份 ID（最新在前）。
    pub(super) retained: Vec<String>,
    /// 本次按保留策略删除的基础备份 ID。
    pub(super) removed: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct BaseUploaded {
    pub(super) backup_id: String,
    pub(super) directory: String,
    pub(super) archive: BackupObject,
    pub(super) manifest: BackupObject,
    pub(super) completion: BackupObject,
}

/// 远端完成记录引用的对象摘要；不带本次上传状态，重试可生成完全相同字节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BaseObject {
    pub(super) key: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

/// 远端 `completed.json` 的内容；`created_unix_seconds` 取基础备份完成时刻，保证幂等。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BaseUploadRecord {
    pub(super) format: String,
    pub(super) backup_id: String,
    pub(super) created_unix_seconds: u64,
    pub(super) archive: BaseObject,
    pub(super) manifest: BaseObject,
}

impl Report {
    pub(super) fn record(&mut self, segment_name: &str, object: BackupObject) {
        let entry = Uploaded {
            segment_name: segment_name.to_owned(),
            key: object.key,
            bytes: object.bytes,
            sha256: object.sha256,
        };
        match object.status {
            BackupObjectStatus::Uploaded => self.uploaded.push(entry),
            BackupObjectStatus::AlreadyPresent => self.already_present.push(entry),
        }
    }
}

impl BaseReport {
    pub(super) fn record(&mut self, object: BaseUploaded) {
        // 以 archive 的上传状态代表整组结果：三份对象都在同一次运行中确认。
        match object.archive.status {
            BackupObjectStatus::Uploaded => self.uploaded.push(object),
            BackupObjectStatus::AlreadyPresent => self.already_present.push(object),
        }
    }
}

impl BaseObject {
    pub(super) fn of(object: &BackupObject) -> Self {
        Self { key: object.key.clone(), bytes: object.bytes, sha256: object.sha256.clone() }
    }
}
