// 主机侧（由 `auction-cli alert report` 读取）告警来源的事实投影与阈值评估。
//
// Worker 运行在容器/服务边界内，无法读取备份目录的文件系统水位、定时备份回执或 WAL
// 暂存目录；伪造一条“来源不可用”会掩盖真实主机状态。因此这两类来源只由 CLI 在主机上
// 采集可复核事实后提交，Worker 的完整来源闭集（`OperationsAlertSource::WORKER`）不变。
//
// 事实只来自两类证据：`fstatvfs` 的非特权可用块，以及已经 fsync 发布的不可变回执/
// manifest。这里不解析 journal、不推断缺失证据，也不把“从未运行”当作失败。

use super::model::OperationsAlertSourceObservation;
use super::signals::{
    OPERATIONS_ALERT_RESTORE_DRILL_MAX_AGE, OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE,
    OperationsAlertError, OperationsAlertKind, OperationsAlertSignal, OperationsAlertSource,
    reached,
};

/// 备份目录文件系统的水位事实；两个下限与定时备份的磁盘门禁使用同一配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertHostDisk {
    pub total_bytes: u64,
    pub available_bytes: u64,
    /// `[backup].min_free_bytes`。
    pub min_free_bytes: u64,
    /// `[backup].min_free_percent`。
    pub min_free_percent: u8,
}

/// 定时备份最近一次终态回执的结果；只有 `completed` 表示成功。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationsAlertHostBackupOutcome {
    Completed,
    Failed,
    Interrupted,
}

impl OperationsAlertHostBackupOutcome {
    /// 返回 `auction-backup-run.v1` 的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    /// 从回执的 `outcome` 名称解析；`running` 与未知取值都不是终态证据。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "interrupted" => Some(Self::Interrupted),
            _ => None,
        }
    }
}

/// 最近一次终态备份运行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertHostBackupRun {
    pub outcome: OperationsAlertHostBackupOutcome,
    /// 距 `finished_at` 的秒数；两个时间点都来自同一个回执。
    pub age_seconds: i64,
}

/// 决策 31 恢复演练证据：运维存放演练回执的目录中最新一次成功演练。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OperationsAlertHostRestoreDrill {
    /// 本次报告未检查演练证据（未提供目录）；不产生也不关闭演练告警的判断依据。
    #[default]
    NotChecked,
    /// 目录不可读：保留已确认异常，但观察不完整，不能关闭历史告警。
    Unreadable,
    /// 已读取目录；`None` 表示没有任何成功演练回执，否则为距完成时刻的秒数。
    Known { latest_success_age_seconds: Option<i64> },
}

/// 备份来源的完整主机事实。
///
/// `disk`/`latest_run` 为 `None`、或演练证据不可读，都表示该项证据缺失：观察保持“来源
/// 不完整”，已确认的异常仍然上报，但不会因为缺少证据而把历史告警标记为已恢复。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OperationsAlertHostBackupFacts {
    pub disk: Option<OperationsAlertHostDisk>,
    pub latest_run: Option<OperationsAlertHostBackupRun>,
    pub restore_drill: OperationsAlertHostRestoreDrill,
}

/// WAL 暂存目录事实：已发布 manifest 且尚未上传的段数量与最老年龄。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OperationsAlertHostWalFacts {
    pub staged_segments: i64,
    pub oldest_staged_age_seconds: i64,
}

/// 备份来源的固定 scope。
const HOST_BACKUP_SCOPE: &str = "backup";
/// WAL 来源的固定 scope。
const HOST_WAL_SCOPE: &str = "wal";

/// 把备份事实转换为 `backup` 来源观察。
///
/// 不变量：
/// - 磁盘门禁与定时备份共用同一配置：字节下限与比例下限必须同时满足才不算偏低；
/// - 只有 `failed`/`interrupted` 终态才形成 `backup_failed`，`completed` 表示恢复正常；
/// - 已检查的演练证据超过 35 天或从未有成功记录时形成 `restore_drill_overdue`（从未记录
///   时 count 为 0）；未检查时不判断，并把该种类标为未检查，存储层不得据缺席关闭它；
/// - 证据不完整时保留已确认异常，但观察不可用，存储层不得据此关闭历史告警。
pub fn evaluate_host_backup(
    facts: &OperationsAlertHostBackupFacts,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let mut signals = Vec::new();
    if let Some(disk) = facts.disk {
        if disk.min_free_bytes == 0 || !(1..=50).contains(&disk.min_free_percent) {
            return Err(OperationsAlertError::Invalid);
        }
        let percent_required =
            (u128::from(disk.total_bytes) * u128::from(disk.min_free_percent)).div_ceil(100);
        let required_bytes =
            disk.min_free_bytes.max(u64::try_from(percent_required).unwrap_or(u64::MAX));
        // 总大小为 0 说明文件系统快照无效：与定时备份的磁盘门禁相同，按不足处理。
        if disk.total_bytes == 0 || disk.available_bytes < required_bytes {
            let shortfall = required_bytes.saturating_sub(disk.available_bytes);
            signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::DiskCapacityLow,
                HOST_BACKUP_SCOPE,
                OperationsAlertSource::Backup,
                i64::try_from(shortfall).unwrap_or(i64::MAX),
                0,
            )?);
        }
    }
    if let Some(run) = facts.latest_run {
        if run.age_seconds < 0 {
            return Err(OperationsAlertError::Invalid);
        }
        if run.outcome != OperationsAlertHostBackupOutcome::Completed {
            signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::BackupFailed,
                HOST_BACKUP_SCOPE,
                OperationsAlertSource::Backup,
                1,
                run.age_seconds,
            )?);
        }
    }
    if let OperationsAlertHostRestoreDrill::Known { latest_success_age_seconds } =
        facts.restore_drill
    {
        if latest_success_age_seconds.is_some_and(|age| age < 0) {
            return Err(OperationsAlertError::Invalid);
        }
        let overdue = latest_success_age_seconds
            .is_none_or(|age| reached(age, OPERATIONS_ALERT_RESTORE_DRILL_MAX_AGE));
        if overdue {
            signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::RestoreDrillOverdue,
                HOST_BACKUP_SCOPE,
                OperationsAlertSource::Backup,
                i64::from(latest_success_age_seconds.is_some()),
                latest_success_age_seconds.unwrap_or(0),
            )?);
        }
    }
    let complete = facts.disk.is_some()
        && facts.latest_run.is_some()
        && facts.restore_drill != OperationsAlertHostRestoreDrill::Unreadable;
    if complete {
        let observation =
            OperationsAlertSourceObservation::available(OperationsAlertSource::Backup, signals)?;
        if facts.restore_drill == OperationsAlertHostRestoreDrill::NotChecked {
            observation.with_unchecked(OperationsAlertKind::RestoreDrillOverdue)
        } else {
            Ok(observation)
        }
    } else {
        let mut observation =
            OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Backup)?;
        observation.signals.extend(signals);
        observation.validate()?;
        Ok(observation)
    }
}

/// 把 WAL 暂存事实转换为 `wal` 来源观察。
///
/// 只有最老暂存段达到 `OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE` 才形成积压信号：上传器每
/// 5 分钟运行一次，短时暂存是正常现象；没有暂存段等同于链路已排空。
pub fn evaluate_host_wal(
    facts: &OperationsAlertHostWalFacts,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    if facts.staged_segments < 0
        || facts.oldest_staged_age_seconds < 0
        || (facts.staged_segments == 0 && facts.oldest_staged_age_seconds != 0)
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    if facts.staged_segments > 0
        && reached(facts.oldest_staged_age_seconds, OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE)
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::WalUploadStalled,
            HOST_WAL_SCOPE,
            OperationsAlertSource::Wal,
            facts.staged_segments,
            facts.oldest_staged_age_seconds,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Wal, signals)
}
