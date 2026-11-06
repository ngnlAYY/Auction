//! Worker 七天窗口 Gauge；历史记录不重复送入 Histogram。

use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use auction_application::runtime_switches::RuntimeSwitches;
use auction_application::sli::{SliSnapshot, SliSnapshotError};
use prometheus::{Histogram, HistogramOpts, IntCounterVec, Opts};

use super::{MetricsError, ProcessMetrics};

mod snapshot;

struct LoadedSnapshot {
    value: SliSnapshot,
    started: Instant,
}

type Snapshot = Mutex<Option<LoadedSnapshot>>;

struct Metrics {
    snapshot: Arc<Snapshot>,
    refreshes: IntCounterVec,
    duration: Histogram,
}

/// 更新纯内存完整快照；失败时保留原值及其原始年龄。
#[derive(Clone, Default)]
pub struct SliMetrics(Option<Arc<Metrics>>);

impl SliMetrics {
    /// 只由 Worker 注册；scrape 只读取内存及同源开关快照。
    pub fn register(
        process: &ProcessMetrics,
        switches: RuntimeSwitches,
    ) -> Result<Self, MetricsError> {
        let Some(registry) = process.enabled_registry() else { return Ok(Self::default()) };
        let snapshot = Arc::new(Mutex::new(None));
        let refreshes = IntCounterVec::new(
            Opts::new("auction_sli_snapshot_refreshes_total", "只读汇总轮次与固定结果。"),
            &["outcome"],
        )
        .map_err(MetricsError::Registry)?;
        for outcome in ["success", "busy", "timeout", "limited", "storage", "invalid_data"] {
            refreshes.with_label_values(&[outcome]);
        }
        let duration = Histogram::with_opts(
            HistogramOpts::new(
                "auction_sli_snapshot_refresh_duration_seconds",
                "实际汇总轮次耗时，不是历史业务耗时。",
            )
            .buckets(vec![0.001, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0]),
        )
        .map_err(MetricsError::Registry)?;
        let collector = snapshot::Collector::new(Arc::clone(&snapshot), switches)?;
        registry.register(Box::new(collector)).map_err(MetricsError::Registry)?;
        registry.register(Box::new(refreshes.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(duration.clone())).map_err(MetricsError::Registry)?;
        Ok(Self(Some(Arc::new(Metrics { snapshot, refreshes, duration }))))
    }

    /// 只有完整成功轮次替换快照，不把截断或失败样本发布为零。
    pub fn publish(&self, snapshot: SliSnapshot, started: Instant) {
        if let Some(metrics) = &self.0 {
            *lock(&metrics.snapshot) = Some(LoadedSnapshot { value: snapshot, started });
            metrics.refreshes.with_label_values(&["success"]).inc();
            metrics.duration.observe(started.elapsed().as_secs_f64());
        }
    }

    /// 失败不改快照，不改业务状态或 readiness。
    pub fn failed(&self, error: SliSnapshotError, started: Instant) {
        if let Some(metrics) = &self.0 {
            metrics.refreshes.with_label_values(&[error.label()]).inc();
            metrics.duration.observe(started.elapsed().as_secs_f64());
        }
    }
}

fn lock(snapshot: &Snapshot) -> MutexGuard<'_, Option<LoadedSnapshot>> {
    match snapshot.lock() {
        Ok(snapshot) => snapshot,
        Err(poison) => poison.into_inner(),
    }
}
