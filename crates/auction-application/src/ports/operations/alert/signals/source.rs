//! 告警来源闭集：Worker 扫描与主机侧报告的来源划分、持久化名称与处置手册锚点。

use std::fmt;

use serde::Serialize;

/// 告警来源闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OperationsAlertSource {
    Process,
    Delivery,
    Fulfillment,
    Recovery,
    /// 收件轮询（MoogleMail 收件扫描）停滞。
    Mail,
    /// 外部凭据失效；同一凭据种类只有一个当前行。
    Credential,
    /// 生命周期与代理到期推进逾期。
    Lifecycle,
    /// 论坛发布与私信投递失败。
    Forum,
    /// 非结算通知投递失败（含邮资不足）。
    Notification,
    /// 主机侧备份证据：备份目录容量与最近一次定时备份终态。
    /// 由 `auction-cli alert report` 在主机上读取回执与文件系统，不进入 Worker 扫描。
    Backup,
    /// 主机侧 WAL 归档证据：暂存目录里超过时限仍未上传的段。
    /// 同样由 `auction-cli alert report` 单独观察。
    Wal,
    /// 资产对账：最近追加的装备持有与偿付能力报告。
    Reconciliation,
}

impl OperationsAlertSource {
    /// Worker 每轮扫描必须提交的来源闭集，顺序即 `OperationsAlertObserveCommand` 的校验顺序。
    ///
    /// 主机侧来源不由 Worker 观察：Worker 无法读取主机文件系统，伪造一条“来源不可用”会
    /// 掩盖真实状态；它们由 CLI 单独提交 `OperationsAlertHostObserveCommand`。
    pub const WORKER: [Self; 10] = [
        Self::Process,
        Self::Delivery,
        Self::Fulfillment,
        Self::Recovery,
        Self::Mail,
        Self::Credential,
        Self::Lifecycle,
        Self::Forum,
        Self::Notification,
        Self::Reconciliation,
    ];

    /// 主机侧来源闭集，顺序即 `OperationsAlertHostObserveCommand` 的校验顺序。
    pub const HOST: [Self; 2] = [Self::Backup, Self::Wal];

    /// 持久化来源闭集（Worker 扫描 + 主机侧报告）；与迁移 005/020/085/091 的
    /// `operations_alert_signals.source` CHECK 一致。
    pub const ALL: [Self; 12] = [
        Self::Process,
        Self::Delivery,
        Self::Fulfillment,
        Self::Recovery,
        Self::Mail,
        Self::Credential,
        Self::Lifecycle,
        Self::Forum,
        Self::Notification,
        Self::Backup,
        Self::Wal,
        Self::Reconciliation,
    ];

    /// 返回数据库和日志使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Delivery => "delivery",
            Self::Fulfillment => "fulfillment",
            Self::Recovery => "recovery",
            Self::Mail => "mail",
            Self::Credential => "credential",
            Self::Lifecycle => "lifecycle",
            Self::Forum => "forum",
            Self::Notification => "notification",
            Self::Backup => "backup",
            Self::Wal => "wal",
            Self::Reconciliation => "reconciliation",
        }
    }

    /// 从持久化名称解析；未知名称返回 `None`，调用方不得回退到默认来源。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.as_str() == value)
    }

    /// 处置手册锚点：在 `ops/README.md` 为每个来源固定 `<a id>`，告警台直接跳转。
    pub const fn runbook_anchor(self) -> &'static str {
        match self {
            Self::Process => "alert-runbook-process",
            Self::Delivery => "alert-runbook-delivery",
            Self::Fulfillment => "alert-runbook-fulfillment",
            Self::Recovery => "alert-runbook-recovery",
            Self::Mail => "alert-runbook-mail",
            Self::Credential => "alert-runbook-credential",
            Self::Lifecycle => "alert-runbook-lifecycle",
            Self::Forum => "alert-runbook-forum",
            Self::Notification => "alert-runbook-notification",
            Self::Backup => "alert-runbook-backup",
            Self::Wal => "alert-runbook-wal",
            Self::Reconciliation => "alert-runbook-reconciliation",
        }
    }
}

impl fmt::Display for OperationsAlertSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
