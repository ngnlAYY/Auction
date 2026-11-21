//! 迁移演练回执逐段验证。

use std::collections::BTreeSet;

use anyhow::Context;
use auction_time::Timestamp;
use jiff::civil;

use super::{DrillEvidence, DrillReceipt, FORMAT, Requirements};
use crate::support::{digest::is_lower_hex, text};
impl DrillReceipt {
    /// 按回执段依次核对；顺序与各段的拒绝条件不变，任一段不符即拒绝部署。
    pub(super) async fn validate(
        &self,
        requirements: &Requirements<'_>,
    ) -> anyhow::Result<DrillEvidence> {
        let date = self.validate_identity(requirements)?;
        self.validate_artifact(requirements)?;
        self.validate_backup(requirements.current_version)?;
        self.validate_postgres()?;
        self.validate_restore()?;
        self.validate_migration(requirements)?;
        self.validate_readiness()?;
        self.validate_consistency()?;
        let minimum = self.validate_downtime(requirements.protection_window_minutes)?;
        self.validate_conclusion()?;
        Ok(DrillEvidence {
            drill_id: self.drill_id.clone(),
            before_version: requirements.current_version,
            after_version: requirements.expected_version,
            minimum_window_minutes: minimum,
            date_utc: date,
        })
    }

    /// 格式、标识、执行人与审核人分离，以及演练日期不在未来、不超过最大年龄。
    fn validate_identity(&self, requirements: &Requirements<'_>) -> anyhow::Result<civil::Date> {
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
        let date = auction_time::parse_date(&self.date_utc)
            .context("迁移演练回执的 date_utc 必须是 YYYY-MM-DD")?;
        let start = Timestamp::from_utc_datetime(date.at(0, 0, 0, 0))
            .context("迁移演练回执日期无法表示")?;
        if start > requirements.now {
            anyhow::bail!("迁移演练回执的日期在未来");
        }
        let max_age = auction_time::Duration::days(i64::from(requirements.max_age_days) + 1);
        if requirements.now - start > max_age {
            anyhow::bail!(
                "迁移演练回执超过 {} 天，不能证明本次迁移区间；请重做演练",
                requirements.max_age_days
            );
        }
        let (Some(started), Some(finished)) = (self.started_at, self.finished_at) else {
            anyhow::bail!("迁移演练回执缺少有效的开始与完成时间");
        };
        if finished < started
            || started < start
            || finished >= start + auction_time::Duration::days(1)
            || finished > requirements.now
        {
            anyhow::bail!("迁移演练回执的时间与 date_utc 或当前时间不一致");
        }
        Ok(date)
    }

    /// 演练所用制品必须就是本次候选：提交、包名与 CLI 摘要逐项一致。
    fn validate_artifact(&self, requirements: &Requirements<'_>) -> anyhow::Result<()> {
        let artifact = requirements.artifact;
        if self.artifact.commit != artifact.source_commit {
            anyhow::bail!("迁移演练回执的候选提交与本次制品不一致");
        }
        if self.artifact.package != artifact.package {
            anyhow::bail!("迁移演练回执的制品名称与本次发布包不一致");
        }
        if !is_lower_hex(&self.artifact.package_sha256, 64)
            || self.artifact.package_sha256 != requirements.package_sha256
            || self.artifact.cli_sha256 != artifact.cli_sha256()
        {
            anyhow::bail!("迁移演练回执的制品摘要与本次候选 CLI 不一致");
        }
        validate_text("artifact.version", &self.artifact.version, 64)?;
        validate_text("artifact.previous_version", &self.artifact.previous_version, 256)?;
        if self.artifact.previous_version != requirements.previous_package {
            anyhow::bail!("迁移演练回执的旧制品包与当前已安装角色身份不一致");
        }
        if !is_lower_hex(&self.artifact.commit, 40) {
            anyhow::bail!("迁移演练回执的候选提交格式无效");
        }
        let Some(commit_prefix) = self.artifact.commit.get(..12) else {
            anyhow::bail!("迁移演练回执的候选提交格式无效");
        };
        let version_prefix = format!("auction-rust-{}-", self.artifact.version);
        if !self.artifact.package.starts_with(&version_prefix)
            || !self.artifact.package.ends_with(commit_prefix)
        {
            anyhow::bail!("迁移演练回执版本与候选制品包名或提交不一致");
        }
        Ok(())
    }

    /// 源备份已校验、schema 版本等于当前数据库，archive 与 manifest 各恰好一条。
    fn validate_backup(&self, current_version: i64) -> anyhow::Result<()> {
        if self.backup.verify_result != "verified" {
            anyhow::bail!("迁移演练回执的备份未通过 backup verify");
        }
        if self.backup.schema_version != Some(current_version) {
            anyhow::bail!("迁移演练回执的备份 schema 版本与当前数据库（{current_version}）不一致");
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
        Ok(())
    }

    /// 演练必须在隔离的 PostgreSQL 18 实例上进行。
    fn validate_postgres(&self) -> anyhow::Result<()> {
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
        Ok(())
    }

    fn validate_restore(&self) -> anyhow::Result<()> {
        if self.restore.result != "passed"
            || !self
                .restore
                .elapsed_seconds
                .is_some_and(|seconds| seconds.is_finite() && seconds >= 0.0)
            || self.restore.receipt.trim().is_empty()
            || self.restore.preflight_evidence.trim().is_empty()
        {
            anyhow::bail!("迁移演练回执的恢复未标记通过或缺少有效证据");
        }
        Ok(())
    }

    /// 迁移区间必须恰好是“当前数据库 → 候选期望版本”，结果通过并附耗时与证据。
    fn validate_migration(&self, requirements: &Requirements<'_>) -> anyhow::Result<()> {
        let before = self.migration.before_version;
        let after = self.migration.after_version;
        let applied = self.migration.applied;
        let expected_applied = requirements
            .expected_version
            .checked_sub(requirements.current_version)
            .context("迁移版本区间超出支持范围")?;
        if self.migration.result != "passed"
            || self.migration.exit_code != Some(0)
            || before != Some(requirements.current_version)
            || after != Some(requirements.expected_version)
            || applied != Some(expected_applied)
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
        let (Some(started), Some(finished)) =
            (self.migration.started_at, self.migration.finished_at)
        else {
            anyhow::bail!("迁移演练回执缺少有效的迁移起止时间");
        };
        if finished < started {
            anyhow::bail!("迁移演练回执的迁移结束时间早于开始时间");
        }
        let interval = finished
            .timestamp()
            .checked_sub(started.timestamp())
            .context("迁移演练回执的起止时间溢出")? as f64;
        let elapsed = self.migration.elapsed_seconds;
        if !elapsed.is_some_and(|seconds| {
            seconds.is_finite() && seconds >= 0.0 && (seconds - interval).abs() <= 1.0
        }) || self.migration.evidence.trim().is_empty()
            || self.migration.timing_evidence.trim().is_empty()
        {
            anyhow::bail!("迁移演练回执缺少一致的迁移耗时或证据");
        }
        Ok(())
    }

    fn validate_readiness(&self) -> anyhow::Result<()> {
        let migration_finished =
            self.migration.finished_at.context("迁移演练回执缺少迁移完成时间")?;
        let drill_finished = self.finished_at.context("迁移演练回执缺少演练完成时间")?;
        for (role, probe) in [
            ("worker", &self.readiness.worker),
            ("admin", &self.readiness.admin),
            ("public", &self.readiness.public),
        ] {
            let Some(checked_at) = probe.checked_at else {
                anyhow::bail!("迁移演练回执的 {role} readiness 缺少检查时间");
            };
            if probe.result != "passed"
                || checked_at < migration_finished
                || checked_at > drill_finished
                || probe.evidence.trim().is_empty()
                || probe.exit_code != Some(0)
                || probe.pid.is_none_or(|pid| pid <= 0)
            {
                anyhow::bail!("迁移演练回执的 {role} readiness 未标记为演练后的通过结果");
            }
            if probe.http_status != Some(200) {
                anyhow::bail!("迁移演练回执的 {role} readiness 未记录 HTTP 200");
            }
        }
        if self.readiness.budget_seconds_per_role <= 0 {
            anyhow::bail!("迁移演练回执的 readiness 预算无效");
        }
        Ok(())
    }

    /// 演练必须实际执行 `auction-cli db inspect-consistency` 并附证据，not_available 不通过。
    fn validate_consistency(&self) -> anyhow::Result<()> {
        if self.consistency_check.work_item != "W11.4"
            || self.consistency_check.result != "passed"
            || self
                .consistency_check
                .evidence
                .as_deref()
                .is_none_or(|evidence| evidence.trim().is_empty())
            || self.consistency_check.note.trim().is_empty()
        {
            anyhow::bail!("迁移演练回执必须附 W11.4 一致性巡检通过证据");
        }
        Ok(())
    }

    /// 停机时间线单调，窗口下限不低于停机时长的三倍，本次保护窗口不低于该下限；返回下限。
    fn validate_downtime(&self, protection_window_minutes: u32) -> anyhow::Result<u32> {
        if self.downtime.estimate_basis.trim().is_empty() {
            anyhow::bail!("迁移演练回执缺少停机估算依据");
        }
        let (started, _drained, ready) =
            match (self.downtime.started_at, self.downtime.drained_at, self.downtime.ready_at) {
                (Some(started), Some(drained), Some(ready))
                    if drained >= started && ready >= drained =>
                {
                    (started, drained, ready)
                }
                _ => anyhow::bail!("迁移演练回执缺少完整的停机时间线"),
            };
        let measured = self.downtime.measured_seconds;
        let estimated = self.downtime.estimated_seconds;
        if measured.is_none() && estimated.is_none() {
            anyhow::bail!("迁移演练回执缺少实测或估算的停机时长");
        }
        if estimated.is_some_and(|seconds| !seconds.is_finite() || seconds <= 0.0) {
            anyhow::bail!("迁移演练回执的估算停机时长无效");
        }
        let measured_interval = ready
            .timestamp()
            .checked_sub(started.timestamp())
            .context("迁移演练回执的停机时间线溢出")? as f64;
        if !measured.is_some_and(|seconds| {
            seconds.is_finite() && seconds > 0.0 && (seconds - measured_interval).abs() <= 1.0
        }) && measured.is_some()
        {
            anyhow::bail!("迁移演练回执的实测停机时长与时间线不一致");
        }
        let seconds = measured.into_iter().chain(estimated).fold(0.0_f64, f64::max);
        let required_window = (seconds / 20.0).ceil();
        if !seconds.is_finite()
            || seconds <= 0.0
            || !required_window.is_finite()
            || required_window >= i64::MAX as f64
        {
            anyhow::bail!("迁移演练回执的停机时长超出支持范围");
        }
        let expected_minimum = required_window as i64;
        let minimum = self.downtime.minimum_window_minutes.unwrap_or(expected_minimum);
        if minimum < expected_minimum || minimum < 1 {
            anyhow::bail!("迁移演练回执的 minimum_window_minutes 低于停机时长的三倍");
        }
        let selected =
            self.downtime.selected_window_minutes.context("迁移演练回执缺少所选保护窗口")?;
        if selected < minimum {
            anyhow::bail!("迁移演练回执的所选保护窗口低于窗口下限");
        }
        let minimum = u32::try_from(minimum).context("迁移演练回执的窗口下限超出范围")?;
        if protection_window_minutes < minimum {
            anyhow::bail!(
                "--protection-window-minutes（{protection_window_minutes}）小于演练回执要求的 \
                 {minimum} 分钟"
            );
        }
        Ok(minimum)
    }

    /// 证据清单摘要有效、没有未解释的异常且结论为 passed。
    fn validate_conclusion(&self) -> anyhow::Result<()> {
        let mut names = BTreeSet::new();
        if self.evidence_files.is_empty() {
            anyhow::bail!("迁移演练回执缺少证据文件清单");
        }
        for evidence in &self.evidence_files {
            validate_text("evidence file name", &evidence.name, 256)?;
            if !is_lower_hex(&evidence.sha256, 64) || !names.insert(&evidence.name) {
                anyhow::bail!("迁移演练回执的证据摘要无效或文件名重复");
            }
        }
        if !self.anomalies.is_empty() {
            anyhow::bail!("迁移演练回执记录了未解释的异常，不能作为部署依据");
        }
        if self.conclusion.result != "passed" {
            anyhow::bail!("迁移演练回执的结论不是 passed");
        }
        validate_text("conclusion.summary", &self.conclusion.summary, 4096)
    }
}

fn validate_text(field: &str, value: &str, max: usize) -> anyhow::Result<()> {
    if !text::single_line(value, max) {
        anyhow::bail!("迁移演练回执的 {field} 无效");
    }
    Ok(())
}
