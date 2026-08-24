//! 论坛发布与回帖导入的独立 Worker 循环。
//!
//! 旧版把 forum sync 和 reply scan 放在同一 owner 内，但与 Auction、MoogleMail 等 owner 使用
//! 不同任务。这里保持同一顺序，同时隔离两阶段错误，避免主题发布失败跳过回帖导入，或反向
//! 阻塞其它 Worker 能力。

use std::sync::Arc;

use auction_application::Clock;

use super::{PersistentWorker, poll_delay};

pub(crate) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    loop {
        if let Some(runtime) = &worker.forum_sync {
            let result =
                super::forum_sync::run_cycle(worker.as_ref(), runtime, worker.forum.lease_duration)
                    .await;
            if let Err(error) = result {
                tracing::warn!(error = %error, "论坛主题同步本轮失败；回帖导入仍将继续");
            }
        }

        if let Some(runtime) = &worker.forum_reply {
            let result = super::forum_reply::run_cycle(worker.as_ref(), runtime).await;
            if let Err(error) = result {
                tracing::warn!(error = %error, "论坛回帖导入本轮失败；将在下一轮重试");
            }
        }

        let wait = poll_delay(worker.forum.poll_interval, worker.forum.jitter);
        let scheduled_wake = tokio::time::Instant::now() + wait;
        if super::wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
            return;
        }
    }
}
