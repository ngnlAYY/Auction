//! Admin API 的周期 heartbeat 发布任务。

use std::{sync::Arc, time::Duration};

use auction_observability::HeartbeatPublisher;

/// 启动周期 heartbeat；发布失败只记录告警，不中断 API 进程。
pub(super) fn spawn_heartbeat(publisher: Arc<HeartbeatPublisher>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        // 心跳只报告当前状态；数据库恢复后不补发过期 tick，避免集中查询和写入。
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = publisher.publish_current().await {
                tracing::warn!(error = %error, "admin heartbeat publish failed");
            }
        }
    })
}
