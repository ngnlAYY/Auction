//! 迁移演练回执核对：含迁移的版本必须提供与本次制品、备份 manifest、迁移区间、
//! readiness 与停机预算一致的 `auction-migration-drill.v1` 回执；未知或未完成一律拒绝。

use std::path::Path;

use auction_time::Timestamp;
use jiff::civil;
use serde::Deserialize;

use super::artifact::Artifact;
use crate::support::files;

mod validation;
pub(crate) const FORMAT: &str = "auction-migration-drill.v1";
const MAX_RECEIPT_BYTES: u64 = 2 * 1024 * 1024;

/// 通过核对的最小演练证据；只保留安全标识与区间。
#[derive(Debug, Clone)]
pub(crate) struct DrillEvidence {
    pub(crate) drill_id: String,
    pub(crate) before_version: i64,
    pub(crate) after_version: i64,
    pub(crate) minimum_window_minutes: u32,
    pub(crate) date_utc: civil::Date,
}

/// 本次部署的核对输入；由命令参数与已校验制品组成。
pub(crate) struct Requirements<'a> {
    pub(crate) artifact: &'a Artifact,
    pub(crate) current_version: i64,
    pub(crate) expected_version: i64,
    pub(crate) previous_package: &'a str,
    pub(crate) package_sha256: &'a str,
    pub(crate) protection_window_minutes: u32,
    pub(crate) max_age_days: u32,
    pub(crate) now: Timestamp,
}

pub(crate) async fn inspect(
    path: &Path,
    requirements: &Requirements<'_>,
) -> anyhow::Result<DrillEvidence> {
    if !path.is_absolute() {
        anyhow::bail!("--migration-drill-receipt 必须是绝对路径");
    }
    let bytes = files::read_bounded(path, MAX_RECEIPT_BYTES)
        .map_err(|error| anyhow::anyhow!("读取迁移演练回执失败：{error}"))?;
    let receipt: DrillReceipt = serde_json::from_slice(&bytes)
        .map_err(|error| anyhow::anyhow!("解析迁移演练回执失败：{error}"))?;
    receipt.validate(requirements).await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillReceipt {
    format: String,
    drill_id: String,
    supersedes_drill_id: Option<String>,
    operator: String,
    reviewer: String,
    date_utc: String,
    started_at: Option<Timestamp>,
    finished_at: Option<Timestamp>,
    artifact: DrillArtifact,
    backup: DrillBackup,
    postgres: DrillPostgres,
    restore: DrillRestore,
    migration: DrillMigration,
    readiness: DrillReadiness,
    consistency_check: DrillConsistency,
    downtime: DrillDowntime,
    evidence_files: Vec<DrillEvidenceFile>,
    anomalies: Vec<serde_json::Value>,
    conclusion: DrillConclusion,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillArtifact {
    version: String,
    commit: String,
    package: String,
    package_sha256: String,
    cli_sha256: String,
    previous_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillBackup {
    created_at: Option<Timestamp>,
    schema_version: Option<i64>,
    database_name_sha256: String,
    files: Vec<DrillBackupFile>,
    verify_result: String,
    verify_evidence: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillBackupFile {
    kind: String,
    name: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillPostgres {
    major: i64,
    server_version: Option<String>,
    isolated: bool,
    instance_label: String,
    host: String,
    port: i64,
    database: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillRestore {
    result: String,
    elapsed_seconds: Option<f64>,
    receipt: String,
    preflight_evidence: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillMigration {
    before_version: Option<i64>,
    after_version: Option<i64>,
    expected_version: Option<i64>,
    applied: Option<i64>,
    started_at: Option<Timestamp>,
    finished_at: Option<Timestamp>,
    elapsed_seconds: Option<f64>,
    exit_code: Option<i32>,
    result: String,
    evidence: String,
    timing_evidence: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillReadiness {
    budget_seconds_per_role: i64,
    worker: DrillProbe,
    admin: DrillProbe,
    public: DrillProbe,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillProbe {
    result: String,
    http_status: Option<i64>,
    exit_code: Option<i32>,
    checked_at: Option<Timestamp>,
    pid: Option<i64>,
    evidence: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillConsistency {
    work_item: String,
    result: String,
    evidence: Option<String>,
    note: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillDowntime {
    started_at: Option<Timestamp>,
    drained_at: Option<Timestamp>,
    ready_at: Option<Timestamp>,
    measured_seconds: Option<f64>,
    estimated_seconds: Option<f64>,
    estimate_basis: String,
    minimum_window_minutes: Option<i64>,
    selected_window_minutes: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillEvidenceFile {
    name: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillConclusion {
    result: String,
    summary: String,
}

#[cfg(test)]
mod tests;
