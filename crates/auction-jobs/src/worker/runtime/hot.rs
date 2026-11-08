//! Worker 热读取快照的进程内载体。
//!
//! 周期读取只在校验成功后一次替换整个快照；正在执行的循环继续使用手中的旧 `Arc`，
//! 下一个循环边界读取新快照。因此读方永远看到一份完整、已校验的参数组合，不会出现
//! 半更新的批次、超时或退避（不变量 5：内存只承载节奏，不承载到期或完成状态）。

use std::sync::{Arc, RwLock};

use auction_application::admin::WorkerHotConfig;
use auction_application::mail::{MailReplyPolicy, MailReplyPolicySource};

/// 所有 Worker 循环共享的热参数快照。
#[derive(Clone, Debug)]
pub struct WorkerHotRuntime {
    current: Arc<RwLock<Arc<WorkerHotConfig>>>,
}

impl WorkerHotRuntime {
    /// 从已应用的启动配置建立初始快照。
    pub fn new(config: WorkerHotConfig) -> Self {
        Self { current: Arc::new(RwLock::new(Arc::new(config))) }
    }

    /// 取得当前完整快照；调用方按需在循环内持有到本次处理结束。
    pub fn snapshot(&self) -> Arc<WorkerHotConfig> {
        // 快照只影响节奏；锁中毒时沿用最后一次成功发布的值，不能升级为进程故障。
        Arc::clone(&self.current.read().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// 原子替换整份快照并返回旧值；调用前必须已完成完整候选校验。
    pub fn replace(&self, config: WorkerHotConfig) -> Arc<WorkerHotConfig> {
        let next = Arc::new(config);
        std::mem::replace(
            &mut *self.current.write().unwrap_or_else(|poisoned| poisoned.into_inner()),
            next,
        )
    }

    /// 应用一次读取结果：校验失败保留旧快照并把错误交给调用方告警。
    pub fn apply_reload<E>(
        &self,
        reload: Result<WorkerHotConfig, E>,
    ) -> Result<Arc<WorkerHotConfig>, E> {
        match reload {
            Ok(config) => Ok(self.replace(config)),
            Err(error) => Err(error),
        }
    }
}

impl MailReplyPolicySource for WorkerHotRuntime {
    fn current(&self) -> MailReplyPolicy {
        self.snapshot().mail.reply_policy()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use auction_application::admin::{
        WorkerDeliveryHotConfig, WorkerForumHotConfig, WorkerHotConfig, WorkerMailHotConfig,
    };
    use auction_application::delivery::DeliverySendIntervals;

    use auction_application::mail::MailReplyPolicySource;

    use super::WorkerHotRuntime;

    fn config(marker: u16) -> WorkerHotConfig {
        WorkerHotConfig {
            mail: WorkerMailHotConfig {
                batch_size: marker,
                jitter_seconds: u64::from(marker),
                fetch_attempts: 3,
                fetch_timeout_seconds: 30,
                process_attempts: 3,
                item_timeout_seconds: 30,
                batch_timeout_seconds: 600,
                retry_base_seconds: 1,
                retry_maximum_seconds: 30,
                account_reply_window_seconds: u64::from(marker) + 60,
                explanation_mail_window_seconds: u64::from(marker) + 120,
                explanation_mail_limit: u32::from(marker),
                send_intervals: DeliverySendIntervals::default(),
            },
            forum: WorkerForumHotConfig {
                batch_size: marker,
                poll_interval_seconds: 300,
                jitter_seconds: 5,
                item_timeout_seconds: 240,
            },
            delivery: WorkerDeliveryHotConfig {
                batch_limit: marker,
                poll_interval_seconds: 60,
                jitter_seconds: 5,
                request_timeout_seconds: 180,
                finalize_timeout_seconds: 5,
            },
        }
    }

    #[test]
    fn replace_is_atomic_for_concurrent_readers() {
        let runtime = WorkerHotRuntime::new(config(1));
        let barrier = Arc::new(Barrier::new(3));
        let mut readers = Vec::new();
        for _ in 0..2 {
            let runtime = runtime.clone();
            let barrier = Arc::clone(&barrier);
            readers.push(std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..1_000 {
                    // 每次快照的三个分组必须来自同一代次，不能交错。
                    let snapshot = runtime.snapshot();
                    assert_eq!(snapshot.mail.batch_size, snapshot.forum.batch_size);
                    assert_eq!(snapshot.mail.batch_size, snapshot.delivery.batch_limit);
                }
            }));
        }
        barrier.wait();
        for marker in 2..=20 {
            runtime.replace(config(marker));
        }
        for reader in readers {
            reader.join().unwrap();
        }
        assert_eq!(runtime.snapshot().mail.batch_size, 20);
        let policy = MailReplyPolicySource::current(&runtime);
        assert_eq!(policy.account_reply_window_seconds, 80);
        assert_eq!(policy.explanation_mail_window_seconds, 140);
        assert_eq!(policy.explanation_mail_limit, 20);
    }

    #[test]
    fn failed_reload_keeps_previous_snapshot() {
        let runtime = WorkerHotRuntime::new(config(1));
        let previous = runtime.snapshot();
        let result: Result<WorkerHotConfig, &str> = Err("invalid candidate");
        assert_eq!(runtime.apply_reload(result), Err("invalid candidate"));
        let current = runtime.snapshot();
        assert_eq!(*current, *previous);
        assert_eq!(current.mail.batch_size, 1);
        assert_eq!(MailReplyPolicySource::current(&runtime).account_reply_window_seconds, 61);

        let previous = runtime.apply_reload(Ok::<_, &str>(config(2))).unwrap();
        assert_eq!(previous.mail.batch_size, 1);
        assert_eq!(runtime.snapshot().mail.batch_size, 2);
        assert_eq!(MailReplyPolicySource::current(&runtime).account_reply_window_seconds, 62);
    }
}
