//! Gauge 编码只读取完整内存快照；持锁直到本次采集完成，避免交错版本。

use std::sync::Arc;

use auction_application::{
    RuntimeSwitches, SLI_AWAITING_INDEX, SLI_SLOW_PROXY_FLOW, SliBidKind, SliWorld,
};
use prometheus::{
    Gauge, GaugeVec, IntGauge, IntGaugeVec, Opts,
    core::{Collector as PrometheusCollector, Desc},
    proto::MetricFamily,
};

use super::{MetricsError, Snapshot, lock};

pub(super) struct Collector {
    snapshot: Arc<Snapshot>,
    switches: RuntimeSwitches,
    available: IntGauge,
    enabled: IntGauge,
    last_success: Gauge,
    age: Gauge,
    rate: GaugeVec,
    peak: GaugeVec,
    pending: GaugeVec,
    oldest: GaugeVec,
    flow_available: IntGaugeVec,
    blocked: IntGaugeVec,
}

impl Collector {
    pub(super) fn new(
        snapshot: Arc<Snapshot>,
        switches: RuntimeSwitches,
    ) -> Result<Self, MetricsError> {
        let flow_available = IntGaugeVec::new(
            Opts::new("auction_sli_flow_available", "流程是否有完整的可发布快照。"),
            &["flow"],
        )
        .map_err(MetricsError::Registry)?;
        let blocked = IntGaugeVec::new(
            Opts::new("auction_sli_flow_blocked", "无法评估的流程及固定阻塞原因。"),
            &["flow", "reason"],
        )
        .map_err(MetricsError::Registry)?;
        for flow in SLI_AWAITING_INDEX {
            flow_available.with_label_values(&[flow]).set(0);
            blocked.with_label_values(&[flow, "awaiting_index"]).set(1);
        }
        Ok(Self {
            snapshot,
            switches,
            available: int_gauge("auction_sli_snapshot_available", "是否曾取得完整快照。")?,
            enabled: int_gauge("auction_sli_snapshot_enabled", "Worker 汇总实际开关。")?,
            last_success: gauge(
                "auction_sli_snapshot_last_success_timestamp_seconds",
                "最后成功快照的数据库时间；未成功为零。",
            )?,
            age: gauge("auction_sli_snapshot_age_seconds", "快照单调时钟年龄；未成功为 -1。")?,
            rate: vector("auction_final_bid_commands_per_second", &["world", "kind"])?,
            peak: vector("auction_final_bid_peak_per_second", &["world", "kind"])?,
            pending: vector("auction_sli_pending", &["flow", "world"])?,
            oldest: vector("auction_sli_oldest_pending_seconds", &["flow", "world"])?,
            flow_available,
            blocked,
        })
    }

    fn families(&self) -> [&dyn PrometheusCollector; 10] {
        [
            &self.available,
            &self.enabled,
            &self.last_success,
            &self.age,
            &self.rate,
            &self.peak,
            &self.pending,
            &self.oldest,
            &self.flow_available,
            &self.blocked,
        ]
    }
}

impl PrometheusCollector for Collector {
    fn desc(&self) -> Vec<&Desc> {
        self.families().into_iter().flat_map(PrometheusCollector::desc).collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        let snapshot = lock(&self.snapshot);
        self.enabled.set(i64::from(self.switches.sli_snapshot_enabled()));
        self.available.set(i64::from(snapshot.is_some()));
        self.age.set(
            snapshot.as_ref().map_or(-1.0, |snapshot| snapshot.started.elapsed().as_secs_f64()),
        );
        for flow in ["final_bid_capacity", "slow_proxy_pending"] {
            self.flow_available.with_label_values(&[flow]).set(i64::from(snapshot.is_some()));
        }
        if let Some(snapshot) = snapshot.as_ref() {
            let timestamp = snapshot.value.observed_at.timestamp_micros() as f64 / 1_000_000.0;
            self.last_success.set(timestamp);
            for (world_index, world) in SliWorld::ALL.into_iter().enumerate() {
                for (kind_index, kind) in SliBidKind::ALL.into_iter().enumerate() {
                    let capacity = snapshot.value.final_bids[world_index][kind_index];
                    let labels = [world.label(), kind.label()];
                    self.rate.with_label_values(&labels).set(capacity.per_second);
                    self.peak.with_label_values(&labels).set(capacity.peak_per_second as f64);
                }
                let pending = snapshot.value.slow_pending[world_index];
                let labels = [SLI_SLOW_PROXY_FLOW, world.label()];
                self.pending.with_label_values(&labels).set(pending.count as f64);
                self.oldest.with_label_values(&labels).set(pending.oldest_seconds);
            }
        }
        self.families().into_iter().flat_map(PrometheusCollector::collect).collect()
    }
}

fn int_gauge(name: &str, help: &str) -> Result<IntGauge, MetricsError> {
    IntGauge::with_opts(Opts::new(name, help)).map_err(MetricsError::Registry)
}

fn gauge(name: &str, help: &str) -> Result<Gauge, MetricsError> {
    Gauge::with_opts(Opts::new(name, help)).map_err(MetricsError::Registry)
}

fn vector(name: &str, labels: &[&str]) -> Result<GaugeVec, MetricsError> {
    GaugeVec::new(Opts::new(name, "完整只读快照的窗口或当前值。"), labels)
        .map_err(MetricsError::Registry)
}
