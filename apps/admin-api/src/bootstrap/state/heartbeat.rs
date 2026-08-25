//! Admin API 的周期 heartbeat 发布任务。

use std::{sync::Arc, time::Duration};

use auction_observability::HeartbeatPublisher;

/// 启动周期 heartbeat；发布失败只记录告警，不中断 API 进程。
pub(super) fn spawn_heartbeat(publisher: Arc<HeartbeatPublisher>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = publisher.publish_current().await {
                tracing::warn!(error = %error, "admin heartbeat publish failed");
            }
        }
    })
}
