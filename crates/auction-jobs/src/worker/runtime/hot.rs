//! Worker 热读取快照的进程内载体与启动基线。
//!
//! 周期读取只在校验成功后一次替换整个快照；正在执行的循环继续使用手中的旧 `Arc`，
//! 下一个循环边界读取新快照。因此读方永远看到一份完整、已校验的参数组合，不会出现
//! 半更新的批次、超时或退避（不变量 5：内存只承载节奏，不承载到期或完成状态）。
//! 热字段要与仅重启字段（租约、续租、发送宽限、观察间隔）组合后才是循环实际参数；替换前
//! 由 [`WorkerHotBaseline`] 逐组检查，冲突组保留旧值，循环边界读到的每一组都能直接采用。

use std::sync::{Arc, RwLock};

use auction_application::admin::WorkerHotConfig;
use auction_application::mail::{MailReplyPolicy, MailReplyPolicySource};

use super::config::{DeliveryWorkerRuntimeConfig, ForumWorkerRuntimeConfig};
use super::model::PersistentWorker;
use crate::worker::forum::hot_claim_budget;
use crate::worker::mail::poll::MailPollRuntimeConfig;

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
        Arc::clone(&self.current.read().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// 原子替换整份快照并返回旧值；调用前必须已完成完整候选校验，并经
    /// [`WorkerHotBaseline::reconcile`] 保留与仅重启字段冲突的组。
    pub fn replace(&self, config: WorkerHotConfig) -> Arc<WorkerHotConfig> {
        let next = Arc::new(config);
        std::mem::replace(
            &mut *self.current.write().unwrap_or_else(std::sync::PoisonError::into_inner),
            next,
        )
    }
}

impl MailReplyPolicySource for WorkerHotRuntime {
    fn current(&self) -> MailReplyPolicy {
        self.snapshot().mail.reply_policy()
    }
}

/// 热读取的参数组；名称与 runtime 文档热字段表的分组一致，用作告警字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerHotGroup {
    Mail,
    Forum,
    Delivery,
}

impl WorkerHotGroup {
    /// 告警 `group` 字段使用的组名：`mail`、`forum` 或 `worker.delivery`。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mail => "mail",
            Self::Forum => "forum",
            Self::Delivery => "worker.delivery",
        }
    }
}

/// 热读取的启动基线：交给 runner 的同一份 Worker 装配中、实际运行的循环的启动运行配置，
/// 携带各组的仅重启字段。未运行的循环为 `None`，该组不检查也不告警。只用于替换前的组合检查，
/// 不决定租约、到期或完成状态。
#[derive(Debug, Clone, Copy)]
pub struct WorkerHotBaseline {
    delivery: Option<DeliveryWorkerRuntimeConfig>,
    forum: Option<ForumWorkerRuntimeConfig>,
    /// MoogleMail 轮询器的启动配置；各世界共用同一份预算。
    mail: Option<MailPollRuntimeConfig>,
}

impl WorkerHotBaseline {
    /// 取自交给 runner 的同一份装配，循环是否运行的判定与 runner 相同。
    pub fn for_worker<C>(worker: &PersistentWorker<C>) -> Self {
        Self {
            delivery: worker.delivery_enabled().then_some(worker.delivery),
            forum: worker.forum_enabled().then_some(worker.forum),
            mail: worker.mail_polls.first().map(|poll| poll.config),
        }
    }

    /// 按组检查候选与仅重启字段的组合，判定与各消费端的采用判定相同：消费端会回退的组改用
    /// `current` 中该组的旧值，其余组照常采用候选。返回可发布的快照与保留旧值的组；
    /// 不整体拒绝，避免一组冲突挡住发送间隔等无关修改。
    pub fn reconcile(
        &self,
        current: &WorkerHotConfig,
        candidate: WorkerHotConfig,
    ) -> (WorkerHotConfig, Vec<WorkerHotGroup>) {
        let mut effective = candidate;
        let mut retained = Vec::new();
        // 收件与维护循环的 apply_hot：在启动租约、续租与观察间隔上叠加热预算。
        if let Some(mut mail) = self.mail
            && !mail.apply_hot(&candidate.mail)
        {
            effective.mail = current.mail;
            retained.push(WorkerHotGroup::Mail);
        }
        // forum_runtime 与 sync/reply 领取：轮询边界有效，且单项预算小于启动租约。
        if let Some(forum) = self.forum
            && (!forum.with_hot(&candidate.forum).validate()
                || hot_claim_budget(&candidate.forum, forum.lease_duration).is_none())
        {
            effective.forum = current.forum;
            retained.push(WorkerHotGroup::Forum);
        }
        // delivery_runtime：请求超时须小于启动发送宽限，批次与超时仍在边界内。
        if let Some(delivery) = self.delivery
            && !delivery.with_hot(&candidate.delivery).validate()
        {
            effective.delivery = current.delivery;
            retained.push(WorkerHotGroup::Delivery);
        }
        (effective, retained)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use auction_application::admin::{
        WorkerDeliveryHotConfig, WorkerForumHotConfig, WorkerHotConfig, WorkerMailHotConfig,
    };
    use auction_application::delivery::DeliverySendIntervals;
    use auction_application::mail::MailReplyPolicySource;

    use super::{WorkerHotBaseline, WorkerHotGroup, WorkerHotRuntime};
    use crate::worker::mail::poll::MailPollRuntimeConfig;
    use crate::worker::runtime::config::{DeliveryWorkerRuntimeConfig, ForumWorkerRuntimeConfig};

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
                send_classes: auction_application::mail::MailSendClasses::default(),
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
    fn replace_returns_previous_snapshot() {
        let runtime = WorkerHotRuntime::new(config(1));
        assert_eq!(MailReplyPolicySource::current(&runtime).account_reply_window_seconds, 61);

        let previous = runtime.replace(config(2));
        assert_eq!(previous.mail.batch_size, 1);
        assert_eq!(runtime.snapshot().mail.batch_size, 2);
        assert_eq!(MailReplyPolicySource::current(&runtime).account_reply_window_seconds, 62);
    }

    /// 启动基线：发送宽限 300 秒、论坛租约 300 秒，与 `config` 生成的当前快照相容。
    fn baseline() -> WorkerHotBaseline {
        WorkerHotBaseline {
            delivery: Some(DeliveryWorkerRuntimeConfig {
                sending_grace: Duration::from_secs(300),
                ..DeliveryWorkerRuntimeConfig::default()
            }),
            forum: Some(ForumWorkerRuntimeConfig {
                lease_duration: Duration::from_secs(300),
                ..ForumWorkerRuntimeConfig::default()
            }),
            mail: Some(MailPollRuntimeConfig::default()),
        }
    }

    #[test]
    fn compatible_candidate_replaces_every_group() {
        let current = config(1);
        let candidate = config(2);
        let (effective, retained) = baseline().reconcile(&current, candidate);
        assert_eq!(effective, candidate);
        assert!(retained.is_empty());
    }

    #[test]
    fn groups_conflicting_with_restart_fields_keep_previous_values() {
        let current = config(1);
        let mut candidate = config(2);
        // 请求超时不小于启动发送宽限、论坛单项预算不小于启动租约：两组都只能重启后生效。
        candidate.delivery.request_timeout_seconds = 300;
        candidate.forum.item_timeout_seconds = 300;
        candidate.mail.send_intervals.settlement_seconds = 45;
        let (effective, retained) = baseline().reconcile(&current, candidate);
        assert_eq!(retained, [WorkerHotGroup::Forum, WorkerHotGroup::Delivery]);
        assert_eq!(effective.delivery, current.delivery);
        assert_eq!(effective.forum, current.forum);
        // 无冲突的组照常替换，发送间隔不被其他组的冲突挡住。
        assert_eq!(effective.mail, candidate.mail);

        let runtime = WorkerHotRuntime::new(current);
        runtime.replace(effective);
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.delivery.request_timeout_seconds, 180);
        assert_eq!(snapshot.forum.item_timeout_seconds, 240);
        assert_eq!(snapshot.mail.send_intervals.settlement_seconds, 45);
        assert_eq!(snapshot.mail.batch_size, 2);
    }

    /// 未运行的循环没有基线：冲突组不保留也不告警，避免对不存在的循环提示“需重启生效”。
    #[test]
    fn groups_without_running_loops_are_not_checked() {
        let current = config(1);
        let mut candidate = config(2);
        candidate.delivery.request_timeout_seconds = 300;
        candidate.forum.item_timeout_seconds = 300;
        let idle = WorkerHotBaseline { delivery: None, forum: None, ..baseline() };
        let (effective, retained) = idle.reconcile(&current, candidate);
        assert!(retained.is_empty());
        assert_eq!(effective, candidate);
    }

    #[test]
    fn mail_budget_conflict_is_checked_only_when_mail_polling_runs() {
        let current = config(1);
        let mut candidate = config(2);
        // 单批总预算小于读取与处理预算之和：收件循环的 apply_hot 会拒绝。
        candidate.mail.batch_timeout_seconds = 10;
        let (effective, retained) = baseline().reconcile(&current, candidate);
        assert_eq!(retained, [WorkerHotGroup::Mail]);
        assert_eq!(effective.mail, current.mail);
        assert_eq!(effective.forum, candidate.forum);
        assert_eq!(effective.delivery, candidate.delivery);

        let without_mail = WorkerHotBaseline { mail: None, ..baseline() };
        let (effective, retained) = without_mail.reconcile(&current, candidate);
        assert!(retained.is_empty());
        assert_eq!(effective, candidate);
    }
}
