//! 独立 SLI 唤醒任务；失败只保留旧指标，不参与业务监督或重试。

use std::{sync::Arc, time::Instant};

use auction_application::{RuntimeSwitches, SliSnapshotReader};
use auction_observability::SliMetrics;
use tokio::time::{Duration, MissedTickBehavior};

const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(60);

/// 装配根用 TaskGuard 持有；不向业务退出通道报告观测失败。
pub fn start_sli_snapshot(
    reader: Arc<dyn SliSnapshotReader>,
    switches: RuntimeSwitches,
    metrics: SliMetrics,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SNAPSHOT_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if !switches.sli_snapshot_enabled() {
                continue;
            }
            let started = Instant::now();
            match reader.read_sli_snapshot().await {
                Ok(snapshot) => metrics.publish(snapshot, started),
                Err(error) => {
                    metrics.failed(error, started);
                    tracing::warn!(
                        kind = error.label(),
                        phase = "sli_snapshot",
                        "SLI 汇总未完成；保留旧快照与原年龄"
                    );
                }
            }
        }
    })
}
