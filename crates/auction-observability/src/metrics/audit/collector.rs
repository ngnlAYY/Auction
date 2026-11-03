//! 从进程内审计安全快照导出指标；并发 scrape 不重复累计。

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use prometheus::{
    Gauge, IntCounter, IntCounterVec, Opts,
    core::{Collector, Desc},
    proto::MetricFamily,
};

use super::types::{AuditFailureClass, AuditFailureKind, AuditMetricsSource, AuditSqlstateClass};
use crate::metrics::types::MetricsError;

pub(in crate::metrics) struct AuditMetricsCollector {
    source: Arc<dyn AuditMetricsSource>,
    failures: IntCounterVec,
    blocked: Gauge,
    shadow: IntCounterVec,
    last_failures: Vec<AtomicU64>,
    last_shadow: [AtomicU64; 3],
}

impl AuditMetricsCollector {
    pub(in crate::metrics) fn new(
        source: Arc<dyn AuditMetricsSource>,
    ) -> Result<Self, MetricsError> {
        let shadow = IntCounterVec::new(
            Opts::new(
                "auction_readiness_shadow_mismatch_total",
                "Total shadow audit readiness decisions differing from legacy.",
            ),
            &["class"],
        )
        .map_err(MetricsError::Registry)?;
        for class in AuditFailureClass::ALL {
            shadow.with_label_values(&[class.label()]);
        }
        Ok(Self {
            source,
            failures: IntCounterVec::new(
                Opts::new(
                    "auction_audit_write_failures_total",
                    "Total observed audit SQL failures, including rolled-back retry attempts.",
                ),
                &["class", "sqlstate_class", "kind"],
            )
            .map_err(MetricsError::Registry)?,
            blocked: Gauge::with_opts(Opts::new(
                "auction_audit_readiness_blocked",
                "Whether the configured audit failure policy blocks readiness.",
            ))
            .map_err(MetricsError::Registry)?,
            shadow,
            last_failures: (0..AuditFailureKind::ALL.len() * AuditSqlstateClass::ALL.len())
                .map(|_| AtomicU64::new(0))
                .collect(),
            last_shadow: [const { AtomicU64::new(0) }; 3],
        })
    }
}

fn sync_counter(counter: &IntCounter, last: &AtomicU64, count: i64) {
    let current = u64::try_from(count).unwrap_or(0);
    let previous = last.fetch_max(current, Ordering::Relaxed);
    if current > previous {
        counter.inc_by(current - previous);
    }
}

impl Collector for AuditMetricsCollector {
    fn desc(&self) -> Vec<&Desc> {
        [self.failures.desc(), self.blocked.desc(), self.shadow.desc()]
            .into_iter()
            .flatten()
            .collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        let snapshot = self.source.snapshot();
        self.blocked.set(f64::from(snapshot.readiness_blocked));
        for observation in snapshot.failures {
            let kind = observation.kind;
            let index =
                kind as usize * AuditSqlstateClass::ALL.len() + observation.sqlstate_class as usize;
            sync_counter(
                &self.failures.with_label_values(&[
                    kind.class().label(),
                    observation.sqlstate_class.label(),
                    kind.label(),
                ]),
                &self.last_failures[index],
                observation.count,
            );
        }
        for (index, class) in AuditFailureClass::ALL.into_iter().enumerate() {
            sync_counter(
                &self.shadow.with_label_values(&[class.label()]),
                &self.last_shadow[index],
                snapshot.shadow_mismatches[index],
            );
        }
        [self.failures.collect(), self.blocked.collect(), self.shadow.collect()]
            .into_iter()
            .flatten()
            .collect()
    }
}
