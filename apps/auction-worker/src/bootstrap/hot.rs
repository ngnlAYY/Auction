//! Worker runtime 文件的周期热读取任务。
//!
//! 只在文件长度或修改时间变化时重新读取；候选必须先通过完整文档校验。替换顺序是
//! 先更新共享发送节奏（同一 store clone 的 delivery 调度会立即读取新值），再一次性替换
//! 循环参数快照。任一步失败都保留旧值并告警，不影响现有循环继续运行。

use std::{path::PathBuf, sync::Arc, time::Duration};

use auction_config::RuntimeFileStamp;
use auction_jobs::WorkerHotRuntime;
use auction_process::TaskGuard;
use auction_storage::DatabaseStore;

/// 检测间隔；热参数只影响节奏，不需要秒级生效。
const HOT_CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// 启动周期读取任务；没有 runtime 文件时返回 `None`，完全保持启动配置。
pub(super) fn spawn_hot_config_watch(
    path: PathBuf,
    hot: WorkerHotRuntime,
    store: Arc<DatabaseStore>,
) -> TaskGuard<()> {
    TaskGuard::new(tokio::spawn(async move {
        // 首次 tick 总是完整读取一次，避免启动加载与任务启动之间的更新被代次检查错过。
        let mut last_stamp: Option<RuntimeFileStamp> = None;
        let mut ticker = tokio::time::interval(HOT_CONFIG_POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let Some(stamp) = RuntimeFileStamp::read(&path) else {
                // 文件暂不可读时保留旧参数；下个周期继续观察，不把缺失当成清空配置。
                continue;
            };
            if stamp.unchanged_from(last_stamp.as_ref()) {
                continue;
            }
            // 先记录观察到的代次：无效候选只告警一次，修正文件后再次触发。
            last_stamp = Some(stamp);
            reload(&path, &hot, store.as_ref());
        }
    }))
}

fn reload(path: &std::path::Path, hot: &WorkerHotRuntime, store: &DatabaseStore) {
    let current = hot.snapshot();
    let candidate = match auction_config::load_worker_hot_config(path, &current) {
        Ok(candidate) => candidate,
        Err(error) => {
            // 校验失败保留旧快照；错误只含路径或固定文案，不回显 TOML。
            tracing::warn!(
                error = %error,
                "Worker runtime 热读取候选无效，继续使用上一次有效参数"
            );
            return;
        }
    };
    if candidate == *current {
        return;
    }
    if candidate.mail.send_intervals != current.mail.send_intervals
        && let Err(error) = store.set_delivery_send_intervals(candidate.mail.send_intervals)
    {
        tracing::warn!(
            error = %error,
            "Worker 发送节奏替换失败，保留旧参数"
        );
        return;
    }
    hot.replace(candidate);
    tracing::info!(
        mail_changed = candidate.mail != current.mail,
        forum_changed = candidate.forum != current.forum,
        delivery_changed = candidate.delivery != current.delivery,
        "Worker runtime 热读取参数已原子替换"
    );
}
