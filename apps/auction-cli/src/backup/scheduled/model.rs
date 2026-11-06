//! 定时备份的运行与终态回执闭集；只记录脱敏错误码，不持久化底层错误链或秘密配置。

use std::path::{Path, PathBuf};

use auction_config::BackupConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{disk::DiskSnapshot, filesystem::FileIdentity, offsite::OffsiteRecord};
use crate::backup::{manifest::sha256_bytes, scope::ScopeSummary};

pub(super) const FORMAT: &str = "auction-backup-run.v1";
const MAX_RECORDED_ENTRIES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Running,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Stage {
    Initialize,
    Recover,
    Config,
    PrepareOutput,
    OutputLock,
    DiskBefore,
    OffsitePreflight,
    Create,
    /// 数据库之外的恢复范围：收集、加密并写下配对 sidecar。
    Scope,
    DiskAfter,
    Verify,
    OffsiteEncrypt,
    OffsiteRetry,
    OffsiteDaily,
    OffsiteMonthly,
    OffsiteFinalize,
    Retention,
    Receipt,
}

impl Stage {
    pub(super) fn error_code(self) -> &'static str {
        match self {
            Self::Initialize => "initialize_failed",
            Self::Config => "config_error",
            Self::Recover => "recovery_failed",
            Self::PrepareOutput => "output_directory_invalid",
            Self::OutputLock => "output_lock_busy_or_invalid",
            Self::DiskBefore | Self::DiskAfter => "disk_check_failed",
            Self::Create => "create_failed",
            Self::Scope => "scope_failed",
            Self::Verify => "verify_failed",
            Self::OffsitePreflight => "offsite_preflight_failed",
            Self::OffsiteEncrypt => "offsite_encryption_failed",
            Self::OffsiteRetry => "offsite_retry_failed",
            Self::OffsiteDaily => "offsite_daily_failed",
            Self::OffsiteMonthly => "offsite_monthly_failed",
            Self::OffsiteFinalize => "offsite_state_failed",
            Self::Retention => "retention_failed",
            Self::Receipt => "receipt_failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Failure {
    pub stage: Stage,
    pub error_code: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.error_code)
    }
}

impl std::error::Error for Failure {}

/// 已结束阶段的单调时钟耗时；中断后的未知尾部不按停机时长补算。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StageTiming {
    pub stage: Stage,
    pub elapsed_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArchiveRecord {
    pub archive_file_name: String,
    pub manifest_file_name: String,
    pub archive_sha256: String,
    pub manifest_sha256: String,
    pub schema_version: i64,
    pub created_at: DateTime<Utc>,
    pub archive_identity: FileIdentity,
    pub manifest_identity: FileIdentity,
    pub verified: bool,
    /// 旧 v1 回执没有范围证据；新回执只保存摘要、规模与本机文件身份。
    #[serde(default)]
    pub scope: Option<ScopeSummary>,
}

/// 回执只保存本地策略，不复制 R2 目的地或敏感配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BackupPolicy {
    pub local_retention_count: u16,
    pub min_free_bytes: u64,
    pub min_free_percent: u8,
    pub timeout_seconds: u64,
}

impl From<&BackupConfig> for BackupPolicy {
    fn from(config: &BackupConfig) -> Self {
        Self {
            local_retention_count: config.local_retention_count,
            min_free_bytes: config.min_free_bytes,
            min_free_percent: config.min_free_percent,
            timeout_seconds: config.timeout_seconds,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SkippedEntry {
    pub file_name: String,
    pub reason: String,
}

/// 列表有界，完整数量另记，避免异常目录让回执无界增长。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CleanupRecord {
    pub removed: Vec<String>,
    pub removed_count: u64,
    pub skipped: Vec<SkippedEntry>,
    pub skipped_count: u64,
}

impl CleanupRecord {
    pub(super) fn skip(&mut self, file_name: &str, reason: &str) {
        self.skipped_count = self.skipped_count.saturating_add(1);
        if self.skipped.len() < MAX_RECORDED_ENTRIES {
            self.skipped
                .push(SkippedEntry { file_name: file_name.to_owned(), reason: reason.to_owned() });
        }
    }

    pub(super) fn removed(&mut self, file_name: String) {
        self.removed_count = self.removed_count.saturating_add(1);
        if self.removed.len() < MAX_RECORDED_ENTRIES {
            self.removed.push(file_name);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RunReceipt {
    pub format: String,
    pub run_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub elapsed_millis: u64,
    pub stage_timings: Vec<StageTiming>,
    pub outcome: Outcome,
    pub stage: Stage,
    pub database_name_sha256: String,
    pub output_dir: PathBuf,
    pub policy: Option<BackupPolicy>,
    pub archive: Option<ArchiveRecord>,
    pub disk_before: Option<DiskSnapshot>,
    pub disk_after: Option<DiskSnapshot>,
    pub cleanup: CleanupRecord,
    pub recovered_interrupted: u64,
    /// 固定原因码；旧 v1 回执缺省为空，绝不记录无效文件的原文。
    #[serde(default)]
    pub recovery_warnings: Vec<String>,
    pub failure: Option<Failure>,
    pub heartbeat_configured: bool,
    /// 旧 v1 回执缺省为未启用，继续具备原来的本地清理资格。
    #[serde(default)]
    pub offsite: OffsiteRecord,
    #[serde(skip)]
    clock_started: Option<std::time::Instant>,
}

impl RunReceipt {
    pub(super) fn new(database: &str, output_dir: &Path) -> Self {
        Self {
            format: FORMAT.to_owned(),
            run_id: Uuid::new_v4(),
            started_at: Utc::now(),
            finished_at: None,
            elapsed_millis: 0,
            stage_timings: Vec::new(),
            outcome: Outcome::Running,
            stage: Stage::Initialize,
            database_name_sha256: sha256_bytes(database.as_bytes()),
            output_dir: output_dir.to_owned(),
            policy: None,
            archive: None,
            disk_before: None,
            disk_after: None,
            cleanup: CleanupRecord::default(),
            recovered_interrupted: 0,
            recovery_warnings: Vec::new(),
            failure: None,
            heartbeat_configured: false,
            offsite: OffsiteRecord::default(),
            clock_started: Some(std::time::Instant::now()),
        }
    }

    pub(super) fn archive_name(&self) -> String {
        format!("scheduled-{}-{}.dump", self.started_at.format("%Y%m%dT%H%M%SZ"), self.run_id)
    }

    pub(super) fn receipt_name(&self) -> String {
        format!("{}.json", self.run_id)
    }

    pub(super) fn fail(&mut self, error_code: &str) {
        self.outcome = Outcome::Failed;
        // 继续处理其他副本或总预算耗尽时，首个已知失败不能被后续错误覆盖。
        self.failure.get_or_insert_with(|| Failure {
            stage: self.stage,
            error_code: error_code.to_owned(),
        });
    }

    pub(super) fn enter_stage(&mut self, stage: Stage) {
        self.record_elapsed();
        self.stage = stage;
    }

    pub(super) fn record_elapsed(&mut self) {
        let Some(started) = self.clock_started else {
            return;
        };
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.stage_timings.push(StageTiming {
            stage: self.stage,
            elapsed_millis: elapsed.saturating_sub(self.elapsed_millis),
        });
        self.elapsed_millis = elapsed;
    }
}
