//! 从应用的纯内存快照导出开关指标；标签只有闭集角色与名称。

use super::types::MetricsError;
use auction_application::runtime_switches::{RuntimeSwitchName, RuntimeSwitches};
use prometheus::{
    GaugeVec, IntCounter, IntCounterVec, Opts,
    core::{Collector, Desc},
    proto::MetricFamily,
};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) struct RuntimeSwitchCollector {
    switches: RuntimeSwitches,
    successes: IntCounterVec,
    failures: IntCounterVec,
    age: GaugeVec,
    revisions: GaugeVec,
    values: GaugeVec,
    last_successes: AtomicU64,
    last_failures: AtomicU64,
}

impl RuntimeSwitchCollector {
    pub(super) fn new(switches: RuntimeSwitches) -> Result<Self, MetricsError> {
        Ok(Self {
            switches,
            successes: IntCounterVec::new(
                Opts::new("auction_runtime_switch_refresh_successes_total", "成功刷新累计"),
                &["role"],
            )
            .map_err(MetricsError::Registry)?,
            failures: IntCounterVec::new(
                Opts::new("auction_runtime_switch_refresh_failures_total", "失败刷新累计"),
                &["role"],
            )
            .map_err(MetricsError::Registry)?,
            age: GaugeVec::new(
                Opts::new(
                    "auction_runtime_switch_snapshot_age_seconds",
                    "最后成功快照年龄；尚未成功为 -1",
                ),
                &["role"],
            )
            .map_err(MetricsError::Registry)?,
            revisions: GaugeVec::new(
                Opts::new("auction_runtime_switch_applied_revision", "进程已观察的最新事实版本"),
                &["role", "switch"],
            )
            .map_err(MetricsError::Registry)?,
            values: GaugeVec::new(
                Opts::new(
                    "auction_runtime_switch_effective_value",
                    "bool 为 0/1；三值模式 legacy/shadow/classified 为 0/1/2",
                ),
                &["role", "switch"],
            )
            .map_err(MetricsError::Registry)?,
            last_successes: AtomicU64::new(0),
            last_failures: AtomicU64::new(0),
        })
    }
}

fn sync(counter: &IntCounter, last: &AtomicU64, current: u64) {
    let previous = last.fetch_max(current, Ordering::Relaxed);
    if current > previous {
        counter.inc_by(current - previous);
    }
}

impl Collector for RuntimeSwitchCollector {
    fn desc(&self) -> Vec<&Desc> {
        [
            self.successes.desc(),
            self.failures.desc(),
            self.age.desc(),
            self.revisions.desc(),
            self.values.desc(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
    fn collect(&self) -> Vec<MetricFamily> {
        let status = self.switches.status();
        let role = status.role.label();
        sync(
            &self.successes.with_label_values(&[role]),
            &self.last_successes,
            status.refresh_successes,
        );
        sync(
            &self.failures.with_label_values(&[role]),
            &self.last_failures,
            status.refresh_failures,
        );
        self.age.with_label_values(&[role]).set(status.last_success_age_seconds.unwrap_or(-1.0));
        for view in status.switches.into_iter().filter(|view| view.applies) {
            let labels = [role, view.name.label()];
            self.revisions.with_label_values(&labels).set(view.applied_revision as f64);
            let value = match view.name {
                RuntimeSwitchName::ReadinessAuditFailureMode
                | RuntimeSwitchName::DispositionReadMode => match view.effective_value.as_str() {
                    Some("classified") => 2.0,
                    Some("shadow") => 1.0,
                    _ => 0.0,
                },
                RuntimeSwitchName::ReadinessCacheEnabled
                | RuntimeSwitchName::MetricsSliSnapshotEnabled
                | RuntimeSwitchName::MooglemailPollIsolationEnabled
                | RuntimeSwitchName::MetricsRejectForwardedRequests
                | RuntimeSwitchName::ExternalAccessPaused => {
                    f64::from(view.effective_value.as_bool().unwrap_or(false))
                }
            };
            self.values.with_label_values(&labels).set(value);
        }
        [
            self.successes.collect(),
            self.failures.collect(),
            self.age.collect(),
            self.revisions.collect(),
            self.values.collect(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}
