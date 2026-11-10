//! 由聚合快照派生的指标状态。
//! 判断顺序沿用旧版 Dashboard：先看数据是否可用，再判失败/降级；各面板只消费结果，
//! 保证同一快照在任何面板里得到一致的状态文案。

use auction_web_shared::legacy_t as t;

use crate::api::AdminDashboardSnapshot;

/// 单次渲染内复用的派生状态，避免每个面板各自重复推导同一指标。
pub(super) struct DashboardDerived {
    pub poll_state: &'static str,
    pub poll_status: String,
    pub has_realtime_consumers: bool,
    pub pending: i64,
    pub failures: i64,
    pub outbox_state: &'static str,
    pub outbox_status: String,
    pub audit_failures: i64,
    pub audit_metrics_unavailable: bool,
    pub audit_state: &'static str,
    pub audit_status: String,
    pub fulfillment_backlog: bool,
    pub fulfillment_state: &'static str,
    pub recovery_state: &'static str,
    pub recovery_status: String,
    pub reconciliation_drift: bool,
    pub reconciliation_state: &'static str,
}

impl DashboardDerived {
    pub(super) fn new(snapshot: &AdminDashboardSnapshot) -> Self {
        let mail = snapshot.mail.data.as_ref();
        let (poll_state, poll_status) = match (snapshot.mail.status.as_str(), mail) {
            ("unknown", _) | (_, None) => ("neutral", t!("dashboard-legacy-poll-unknown")),
            (_, Some(data)) if data.pollers.is_empty() => {
                ("neutral", t!("dashboard-legacy-poll-no-pollers"))
            }
            (_, Some(data))
                if data.pollers.iter().any(|poller| poller.consecutive_failures > 0) =>
            {
                ("warn", t!("dashboard-legacy-poll-degraded"))
            }
            (_, Some(data)) if data.pollers.iter().any(|poller| poller.paused) => {
                ("neutral", t!("dashboard-legacy-poll-paused"))
            }
            ("ok", Some(_)) => ("ok", t!("dashboard-legacy-poll-running")),
            _ => ("warn", t!("dashboard-legacy-poll-degraded")),
        };
        let realtime = snapshot.realtime.data.as_ref();
        let has_realtime_consumers = realtime.is_some_and(|data| !data.lags.is_empty());
        let pending = realtime.map_or(0, |data| data.total_lag);
        let failures = realtime.map_or(0, |data| data.failure.count);
        let outbox_state = if realtime.is_none() {
            "neutral"
        } else if failures > 0 {
            "error"
        } else if !has_realtime_consumers {
            "neutral"
        } else if pending > 0 {
            "warn"
        } else {
            "ok"
        };
        let outbox_status = if realtime.is_none() {
            t!("dashboard-legacy-outbox-status-unavailable")
        } else if failures > 0 {
            t!("dashboard-legacy-outbox-status-dead-letters")
        } else if !has_realtime_consumers {
            t!("dashboard-legacy-outbox-status-no-consumers")
        } else if pending > 0 {
            t!("dashboard-legacy-outbox-status-lagging")
        } else {
            t!("dashboard-legacy-outbox-status-ok")
        };
        let fulfillment = snapshot.fulfillment.data.as_ref();
        let recovery = snapshot.recovery.data.as_ref();
        let reconciliation = snapshot.reconciliation.data.as_ref();
        let reconciliation_drift = reconciliation.is_some_and(|data| {
            data.worlds.iter().any(|world| {
                world.custody_missing_count > 0 || world.solvency_shortfall_credits > 0
            })
        });
        let reconciliation_state = if reconciliation.is_none() {
            "neutral"
        } else if reconciliation_drift {
            "error"
        } else {
            "ok"
        };
        let audit_failures = snapshot.health.audit_failure_count;
        let audit_metrics_unavailable = snapshot
            .health
            .missing_services
            .iter()
            .any(|service| service == "public" || service == "worker");
        let audit_state = if audit_failures > 0 {
            "error"
        } else if audit_metrics_unavailable {
            "neutral"
        } else {
            "ok"
        };
        let audit_status = if audit_failures > 0 {
            t!("dashboard-legacy-audit-status-failures")
        } else if audit_metrics_unavailable {
            t!("dashboard-legacy-audit-status-unknown")
        } else {
            t!("dashboard-legacy-audit-status-ok")
        };
        // 状态由服务端按同一告警策略阈值裁决；页面不再用“计数 > 0”建立第二套口径。
        let fulfillment_backlog = snapshot.fulfillment.status == "degraded";
        let fulfillment_state = if fulfillment.is_none() {
            "neutral"
        } else if fulfillment_backlog {
            "error"
        } else if snapshot.fulfillment.status == "ok" {
            "ok"
        } else {
            "neutral"
        };
        let recovery_actionable = snapshot.recovery.status == "degraded";
        let recovery_evidence_unavailable = recovery.is_some_and(|data| {
            data.worlds.iter().any(|world| {
                matches!(world.balance_status.as_str(), "unknown" | "unavailable")
                    || matches!(world.evidence_status.as_str(), "unknown" | "unavailable")
            })
        });
        let recovery_evidence_warning = recovery.is_some_and(|data| {
            data.worlds.iter().any(|world| {
                matches!(world.balance_status.as_str(), "low" | "stale" | "busy")
                    || world.evidence_status == "stale"
            })
        });
        let recovery_state = if recovery.is_none() {
            "neutral"
        } else if recovery_actionable {
            "error"
        } else if recovery_evidence_unavailable {
            "neutral"
        } else if recovery_evidence_warning {
            "warn"
        } else if snapshot.recovery.status == "ok" {
            "ok"
        } else {
            "warn"
        };
        let recovery_status = if recovery.is_none() || recovery_evidence_unavailable {
            t!("dashboard-legacy-recovery-status-unavailable")
        } else if recovery_state == "ok" {
            t!("dashboard-legacy-recovery-status-ok")
        } else {
            t!("dashboard-legacy-recovery-status-degraded")
        };
        Self {
            poll_state,
            poll_status,
            has_realtime_consumers,
            pending,
            failures,
            outbox_state,
            outbox_status,
            audit_failures,
            audit_metrics_unavailable,
            audit_state,
            audit_status,
            fulfillment_backlog,
            fulfillment_state,
            recovery_state,
            recovery_status,
            reconciliation_drift,
            reconciliation_state,
        }
    }
}
