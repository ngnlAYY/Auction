//! 迁移演练回执核对：含迁移的版本必须提供与本次制品、备份 manifest、迁移区间、
//! readiness 与停机预算一致的 `auction-migration-drill.v1` 回执；未知或未完成一律拒绝。

use std::path::Path;

use anyhow::Context;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::Deserialize;

use super::artifact::Artifact;
use crate::support::files;

pub(crate) const FORMAT: &str = "auction-migration-drill.v1";
const MAX_RECEIPT_BYTES: u64 = 2 * 1024 * 1024;

/// 通过核对的最小演练证据；只保留安全标识与区间。
#[derive(Debug, Clone)]
pub(crate) struct DrillEvidence {
    pub(crate) drill_id: String,
    pub(crate) before_version: i64,
    pub(crate) after_version: i64,
    pub(crate) minimum_window_minutes: u32,
    pub(crate) date_utc: NaiveDate,
}

/// 本次部署的核对输入；由命令参数与已校验制品组成。
pub(crate) struct Requirements<'a> {
    pub(crate) artifact: &'a Artifact,
    pub(crate) current_version: i64,
    pub(crate) expected_version: i64,
    pub(crate) protection_window_minutes: u32,
    pub(crate) max_age_days: u32,
    pub(crate) now: DateTime<Utc>,
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
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
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
    created_at: Option<DateTime<Utc>>,
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
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
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
    checked_at: Option<DateTime<Utc>>,
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
    started_at: Option<DateTime<Utc>>,
    drained_at: Option<DateTime<Utc>>,
    ready_at: Option<DateTime<Utc>>,
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

impl DrillReceipt {
    async fn validate(&self, requirements: &Requirements<'_>) -> anyhow::Result<DrillEvidence> {
        if self.format != FORMAT {
            anyhow::bail!("迁移演练回执格式不受支持：{}", self.format);
        }
        validate_text("drill_id", &self.drill_id, 128)?;
        if let Some(supersedes) = &self.supersedes_drill_id {
            validate_text("supersedes_drill_id", supersedes, 128)?;
        }
        validate_text("operator", &self.operator, 128)?;
        validate_text("reviewer", &self.reviewer, 128)?;
        if self.operator == self.reviewer {
            anyhow::bail!("迁移演练回执必须由执行人以外的人审核");
        }
        let date = NaiveDate::parse_from_str(&self.date_utc, "%Y-%m-%d")
            .ok()
            .context("迁移演练回执的 date_utc 必须是 YYYY-MM-DD")?;
        let midnight = date.and_hms_opt(0, 0, 0).context("迁移演练回执日期无法表示")?;
        let start = Utc.from_utc_datetime(&midnight);
        if start > requirements.now {
            anyhow::bail!("迁移演练回执的日期在未来");
        }
        let max_age = chrono::Duration::days(i64::from(requirements.max_age_days) + 1);
        if requirements.now - start > max_age {
            anyhow::bail!(
                "迁移演练回执超过 {} 天，不能证明本次迁移区间；请重做演练",
                requirements.max_age_days
            );
        }
        if let (Some(started), Some(finished)) = (self.started_at, self.finished_at)
            && finished < started
        {
            anyhow::bail!("迁移演练回执的完成时间早于开始时间");
        }

        if self.artifact.commit != requirements.artifact.source_commit {
            anyhow::bail!("迁移演练回执的候选提交与本次制品不一致");
        }
        if self.artifact.package != requirements.artifact.package {
            anyhow::bail!("迁移演练回执的制品名称与本次发布包不一致");
        }
        if !is_lower_hex(&self.artifact.package_sha256, 64)
            || self.artifact.cli_sha256 != requirements.artifact.cli_sha256()
        {
            anyhow::bail!("迁移演练回执的制品摘要与本次候选 CLI 不一致");
        }
        validate_text("artifact.version", &self.artifact.version, 64)?;
        validate_text("artifact.previous_version", &self.artifact.previous_version, 128)?;

        if self.backup.verify_result != "verified" {
            anyhow::bail!("迁移演练回执的备份未通过 backup verify");
        }
        if self.backup.schema_version != Some(requirements.current_version) {
            anyhow::bail!(
                "迁移演练回执的备份 schema 版本与当前数据库（{}）不一致",
                requirements.current_version
            );
        }
        if self.backup.created_at.is_none() || !is_lower_hex(&self.backup.database_name_sha256, 64)
        {
            anyhow::bail!("迁移演练回执的备份证据不完整");
        }
        if self.backup.verify_evidence.trim().is_empty() {
            anyhow::bail!("迁移演练回执缺少备份校验证据名");
        }
        for kind in ["archive", "manifest"] {
            let mut matched = self.backup.files.iter().filter(|file| file.kind == kind);
            let Some(file) = matched.next() else {
                anyhow::bail!("迁移演练回执的备份清单缺少 {kind}");
            };
            if matched.next().is_some() {
                anyhow::bail!("迁移演练回执的备份清单重复记录 {kind}");
            }
            validate_text("backup file name", &file.name, 256)?;
            if !is_lower_hex(&file.sha256, 64) {
                anyhow::bail!("迁移演练回执的备份 {kind} 摘要无效");
            }
        }
        if self
            .backup
            .files
            .iter()
            .any(|file| !matches!(file.kind.as_str(), "archive" | "manifest"))
        {
            anyhow::bail!("迁移演练回执的备份清单含未知条目类型");
        }

        if self.postgres.major != 18
            || !self.postgres.isolated
            || !(1..=65535).contains(&self.postgres.port)
            || self.postgres.host.is_empty()
            || self.postgres.database.is_empty()
            || self.postgres.instance_label.trim().is_empty()
            || !self
                .postgres
                .server_version
                .as_deref()
                .is_some_and(|version| version.starts_with("18."))
        {
            anyhow::bail!("迁移演练回执的 PostgreSQL 18 隔离实例证据无效");
        }

        if self.restore.result != "passed"
            || self.restore.elapsed_seconds.is_none()
            || self.restore.receipt.trim().is_empty()
            || self.restore.preflight_evidence.trim().is_empty()
        {
            anyhow::bail!("迁移演练回执的恢复未标记通过或缺少证据");
        }

        let before = self.migration.before_version;
        let after = self.migration.after_version;
        let applied = self.migration.applied;
        if self.migration.result != "passed"
            || self.migration.exit_code != Some(0)
            || before != Some(requirements.current_version)
            || after != Some(requirements.expected_version)
            || applied != Some(requirements.expected_version - requirements.current_version)
        {
            anyhow::bail!(
                "迁移演练回执的迁移区间必须恰好是 {} → {} 且结果通过",
                requirements.current_version,
                requirements.expected_version
            );
        }
        if self.migration.expected_version != Some(requirements.expected_version) {
            anyhow::bail!("迁移演练回执的期望版本与候选制品不一致");
        }
        if self.migration.elapsed_seconds.is_none()
            || self.migration.evidence.trim().is_empty()
            || self.migration.timing_evidence.trim().is_empty()
        {
            anyhow::bail!("迁移演练回执缺少迁移耗时或证据");
        }
        match (self.migration.started_at, self.migration.finished_at) {
            (Some(started), Some(finished)) if finished >= started => {}
            _ => anyhow::bail!("迁移演练回执缺少有效的迁移起止时间"),
        }

        for (role, probe) in [
            ("worker", &self.readiness.worker),
            ("admin", &self.readiness.admin),
            ("public", &self.readiness.public),
        ] {
            if probe.result != "passed"
                || probe.checked_at.is_none()
                || probe.evidence.trim().is_empty()
                || probe.exit_code != Some(0)
                || probe.pid.is_none_or(|pid| pid <= 0)
            {
                anyhow::bail!("迁移演练回执的 {role} readiness 未标记通过");
            }
            if probe.http_status != Some(200) {
                anyhow::bail!("迁移演练回执的 {role} readiness 未记录 HTTP 200");
            }
        }
        if self.readiness.budget_seconds_per_role <= 0 {
            anyhow::bail!("迁移演练回执的 readiness 预算无效");
        }

        // 已提供 `auction-cli db inspect-consistency`；演练必须实际执行并附证据，
        // 不能再用 not_available 绕过。
        if self.consistency_check.work_item != "W11.4"
            || self.consistency_check.result != "passed"
            || self.consistency_check.evidence.as_deref().is_none_or(str::is_empty)
            || self.consistency_check.note.trim().is_empty()
        {
            anyhow::bail!("迁移演练回执必须附 W11.4 一致性巡检通过证据");
        }

        if self.downtime.estimate_basis.trim().is_empty() {
            anyhow::bail!("迁移演练回执缺少停机估算依据");
        }
        match (self.downtime.started_at, self.downtime.drained_at, self.downtime.ready_at) {
            (Some(started), Some(drained), Some(ready))
                if drained >= started && ready >= drained => {}
            _ => anyhow::bail!("迁移演练回执缺少完整的停机时间线"),
        }
        let measured = self.downtime.measured_seconds;
        let estimated = self.downtime.estimated_seconds;
        if measured.is_none() && estimated.is_none() {
            anyhow::bail!("迁移演练回执缺少实测或估算的停机时长");
        }
        let seconds = measured.into_iter().chain(estimated).fold(0.0_f64, f64::max);
        if !seconds.is_finite() || seconds <= 0.0 {
            anyhow::bail!("迁移演练回执的停机时长无效");
        }
        let minimum = self
            .downtime
            .minimum_window_minutes
            .or_else(|| Some(((seconds * 3.0) / 60.0).ceil() as i64))
            .context("迁移演练回执缺少窗口下限")?;
        let expected_minimum = ((seconds * 3.0) / 60.0).ceil() as i64;
        if minimum < expected_minimum || minimum < 1 {
            anyhow::bail!("迁移演练回执的 minimum_window_minutes 低于停机时长的三倍");
        }
        let selected =
            self.downtime.selected_window_minutes.context("迁移演练回执缺少所选保护窗口")?;
        if selected < minimum {
            anyhow::bail!("迁移演练回执的所选保护窗口低于窗口下限");
        }
        let minimum = u32::try_from(minimum).context("迁移演练回执的窗口下限超出范围")?;
        if requirements.protection_window_minutes < minimum {
            anyhow::bail!(
                "--protection-window-minutes（{}）小于演练回执要求的 {minimum} 分钟",
                requirements.protection_window_minutes
            );
        }

        for evidence in &self.evidence_files {
            validate_text("evidence file name", &evidence.name, 256)?;
            if !is_lower_hex(&evidence.sha256, 64) {
                anyhow::bail!("迁移演练回执的证据摘要无效");
            }
        }
        if !self.anomalies.is_empty() {
            anyhow::bail!("迁移演练回执记录了未解释的异常，不能作为部署依据");
        }
        if self.conclusion.result != "passed" {
            anyhow::bail!("迁移演练回执的结论不是 passed");
        }
        validate_text("conclusion.summary", &self.conclusion.summary, 4096)?;

        Ok(DrillEvidence {
            drill_id: self.drill_id.clone(),
            before_version: requirements.current_version,
            after_version: requirements.expected_version,
            minimum_window_minutes: minimum,
            date_utc: date,
        })
    }
}

fn validate_text(field: &str, value: &str, max: usize) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > max
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        anyhow::bail!("迁移演练回执的 {field} 无效");
    }
    Ok(())
}

pub(crate) fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests;
