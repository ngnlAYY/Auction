//! 外部账号当前进程的只读快照 owner。
//! 只消费既有外部客户端同步事件与显式的维护轮次事件；在内存中累计计数与最近时间，不执行
//! I/O、不改变调用结果，也不保存凭据、URL、页面正文或游标。锁损坏或快照不合法时返回
//! `None`，由调用方显式降级，不能用空闲或零值冒充。

use std::sync::{Arc, Mutex};

use auction_application::external_accounts::{
    ExternalAccountStatus, ExternalAccountStatusReader, ExternalForumAccountStatus,
    ExternalMailAccountStatus, ExternalMaintenanceState, ExternalMaintenanceStatus,
};
use auction_application::external_metrics::{
    ExternalMetricsEvent, ExternalMetricsObserver, ForumRequestCategory, MailMaintenanceOutcome,
    MailOperation, MailRequestCategory, MailSignal, MailWorld,
};
use chrono::{DateTime, Utc};

/// 进程内外部账号快照 owner；克隆共享同一份状态。
#[derive(Clone, Default)]
pub struct ExternalAccountTracker {
    inner: Arc<Mutex<TrackerState>>,
}

#[derive(Default)]
struct TrackerState {
    mail_in_flight: Vec<InFlight>,
    mail_gate_wait_count: u64,
    mail_gate_wait_millis: u64,
    mail_request_count: u64,
    mail_last_request_at: Option<DateTime<Utc>>,
    mail_last_success_at: Option<DateTime<Utc>>,
    mail_last_failure_at: Option<DateTime<Utc>>,
    mail_account_busy_since: Option<DateTime<Utc>>,
    forum_last_request_at: Option<DateTime<Utc>>,
    forum_last_success_at: Option<DateTime<Utc>>,
    forum_last_failure_at: Option<DateTime<Utc>>,
    forum_write_unknown_count: u64,
    maintenance_worlds: Vec<MaintenanceWorldObservation>,
}

#[derive(Clone)]
struct MaintenanceWorldObservation {
    world: MailWorld,
    state: ExternalMaintenanceState,
    failed: bool,
    running_since: Option<DateTime<Utc>>,
    last_event_at: Option<DateTime<Utc>>,
    last_started_at: Option<DateTime<Utc>>,
    last_success_at: Option<DateTime<Utc>>,
    last_failure_at: Option<DateTime<Utc>>,
    last_canceled_at: Option<DateTime<Utc>>,
}

impl MaintenanceWorldObservation {
    fn new(world: MailWorld) -> Self {
        Self {
            world,
            state: ExternalMaintenanceState::Idle,
            failed: false,
            running_since: None,
            last_event_at: None,
            last_started_at: None,
            last_success_at: None,
            last_failure_at: None,
            last_canceled_at: None,
        }
    }
}

/// 单个 (world, operation) 的在途请求；嵌套开始只增加计数并保留最早起点。
struct InFlight {
    world: MailWorld,
    operation: MailOperation,
    count: u32,
}

impl ExternalAccountTracker {
    /// 读取当前快照；未观察到任何事实时仍返回合法的 Idle 投影。
    pub fn snapshot_status(&self) -> Option<ExternalAccountStatus> {
        let observed_at = Utc::now();
        let state = self.inner.lock().ok()?;
        let failed =
            state.maintenance_worlds.iter().filter(|world| world.failed).collect::<Vec<_>>();
        let running_since =
            state.maintenance_worlds.iter().filter_map(|world| world.running_since).min();
        let latest_terminal = state
            .maintenance_worlds
            .iter()
            .filter(|world| world.state != ExternalMaintenanceState::Running)
            .max_by_key(|world| world.last_event_at)
            .map_or(ExternalMaintenanceState::Idle, |world| world.state);
        let aggregate_state = if !failed.is_empty() {
            ExternalMaintenanceState::Failed
        } else if running_since.is_some() {
            ExternalMaintenanceState::Running
        } else {
            latest_terminal
        };
        let last_started_at =
            state.maintenance_worlds.iter().filter_map(|world| world.last_started_at).max();
        let last_success_at =
            state.maintenance_worlds.iter().filter_map(|world| world.last_success_at).max();
        let last_failure_at = failed.iter().filter_map(|world| world.last_failure_at).max();
        let last_canceled_at =
            state.maintenance_worlds.iter().filter_map(|world| world.last_canceled_at).max();
        let status = ExternalAccountStatus {
            observed_at,
            mail: ExternalMailAccountStatus {
                busy: state.mail_account_busy_since.is_some(),
                busy_age_seconds: state
                    .mail_account_busy_since
                    .map(|since| age_seconds(since, observed_at)),
                gate_wait_count: count(state.mail_gate_wait_count),
                gate_wait_seconds_total: count(state.mail_gate_wait_millis / 1000),
                request_count: count(state.mail_request_count),
                last_request_at: state.mail_last_request_at,
                last_success_at: state.mail_last_success_at,
                last_failure_at: state.mail_last_failure_at,
            },
            forum: ExternalForumAccountStatus {
                last_request_at: state.forum_last_request_at,
                last_success_at: state.forum_last_success_at,
                last_failure_at: state.forum_last_failure_at,
                write_unknown_count: count(state.forum_write_unknown_count),
            },
            maintenance: ExternalMaintenanceStatus {
                state: aggregate_state,
                last_started_at,
                last_success_at,
                last_failure_at,
                last_canceled_at,
                running_age_seconds: if failed.is_empty() {
                    running_since.map(|since| age_seconds(since, observed_at))
                } else {
                    None
                },
            },
        };
        status.validate().then_some(status)
    }
}

impl ExternalAccountStatusReader for ExternalAccountTracker {
    fn snapshot(&self) -> Option<ExternalAccountStatus> {
        self.snapshot_status()
    }
}

impl std::fmt::Debug for ExternalAccountTracker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ExternalAccountTracker")
    }
}

impl ExternalMetricsObserver for ExternalAccountTracker {
    fn observe(&self, event: ExternalMetricsEvent) {
        let now = Utc::now();
        let Ok(mut state) = self.inner.lock() else { return };
        state.apply(event, now);
    }
}

impl TrackerState {
    fn maintenance_entry(&mut self, world: MailWorld) -> &mut MaintenanceWorldObservation {
        if let Some(index) = self.maintenance_worlds.iter().position(|entry| entry.world == world) {
            return &mut self.maintenance_worlds[index];
        }
        let index = self.maintenance_worlds.len();
        self.maintenance_worlds.push(MaintenanceWorldObservation::new(world));
        &mut self.maintenance_worlds[index]
    }

    fn apply(&mut self, event: ExternalMetricsEvent, now: DateTime<Utc>) {
        match event {
            ExternalMetricsEvent::MailRequestStarted { world, operation } => {
                self.mail_request_count = self.mail_request_count.saturating_add(1);
                self.mail_last_request_at = Some(now);
                if let Some(entry) = self
                    .mail_in_flight
                    .iter_mut()
                    .find(|entry| entry.world == world && entry.operation == operation)
                {
                    entry.count = entry.count.saturating_add(1);
                } else {
                    self.mail_in_flight.push(InFlight { world, operation, count: 1 });
                }
            }
            ExternalMetricsEvent::MailRequestFinished { world, operation, category } => {
                if let Some(index) = self
                    .mail_in_flight
                    .iter()
                    .position(|entry| entry.world == world && entry.operation == operation)
                {
                    let entry = &mut self.mail_in_flight[index];
                    entry.count = entry.count.saturating_sub(1);
                    let finished = entry.count == 0;
                    if finished {
                        self.mail_in_flight.remove(index);
                    }
                }
                match category {
                    MailRequestCategory::Ok => {
                        self.mail_last_success_at = Some(now);
                        // 成功请求是账号恢复的最强本地证据；清除此前 account_busy 投影。
                        self.mail_account_busy_since = None;
                    }
                    MailRequestCategory::Canceled => {}
                    MailRequestCategory::AccountBusy => {
                        self.mail_last_failure_at = Some(now);
                        self.mail_account_busy_since.get_or_insert(now);
                    }
                    _ => self.mail_last_failure_at = Some(now),
                }
            }
            ExternalMetricsEvent::MailRequestSignal { signal: MailSignal::AccountBusy, .. } => {
                self.mail_account_busy_since.get_or_insert(now);
            }
            ExternalMetricsEvent::MailGateWait { waited, .. } => {
                self.mail_gate_wait_count = self.mail_gate_wait_count.saturating_add(1);
                let millis = u64::try_from(waited.as_millis()).unwrap_or(u64::MAX);
                self.mail_gate_wait_millis = self.mail_gate_wait_millis.saturating_add(millis);
            }
            ExternalMetricsEvent::ForumRequestStarted { .. } => {
                self.forum_last_request_at = Some(now);
            }
            ExternalMetricsEvent::ForumRequestFinished { category, .. } => match category {
                ForumRequestCategory::Ok => self.forum_last_success_at = Some(now),
                ForumRequestCategory::Canceled => {}
                _ => self.forum_last_failure_at = Some(now),
            },
            ExternalMetricsEvent::ForumWriteUnknown { .. } => {
                self.forum_write_unknown_count = self.forum_write_unknown_count.saturating_add(1);
            }
            ExternalMetricsEvent::MailMaintenanceStarted { world } => {
                let entry = self.maintenance_entry(world);
                entry.state = ExternalMaintenanceState::Running;
                entry.running_since = Some(now);
                entry.last_started_at = Some(now);
                entry.last_event_at = Some(now);
            }
            ExternalMetricsEvent::MailMaintenanceFinished { world, outcome } => {
                let entry = self.maintenance_entry(world);
                entry.running_since = None;
                entry.last_event_at = Some(now);
                match outcome {
                    MailMaintenanceOutcome::Succeeded => {
                        entry.state = ExternalMaintenanceState::Succeeded;
                        entry.failed = false;
                        entry.last_success_at = Some(now);
                    }
                    MailMaintenanceOutcome::Failed => {
                        entry.state = ExternalMaintenanceState::Failed;
                        entry.failed = true;
                        entry.last_failure_at = Some(now);
                    }
                    MailMaintenanceOutcome::Canceled => {
                        entry.state = ExternalMaintenanceState::Canceled;
                        entry.last_canceled_at = Some(now);
                    }
                }
            }
            // 其余事件只属于 Prometheus 指标口径，不改变账号快照。
            ExternalMetricsEvent::MailRequestSignal { .. }
            | ExternalMetricsEvent::MailLockWait { .. }
            | ExternalMetricsEvent::MailBudgetExhausted { .. }
            | ExternalMetricsEvent::MailRequestInterval { .. }
            | ExternalMetricsEvent::ForumRequestSignal { .. }
            | ExternalMetricsEvent::ForumGateWait { .. }
            | ExternalMetricsEvent::ForumRequestInterval { .. }
            | ExternalMetricsEvent::ForumTopicScanPages { .. }
            | ExternalMetricsEvent::ForumPmOperation { .. }
            | ExternalMetricsEvent::ForumBudgetExhausted { .. }
            | ExternalMetricsEvent::ForumReplyDelayEstimate { .. }
            | ExternalMetricsEvent::ForumReplyDelaySample { .. }
            | ExternalMetricsEvent::MailNoticeSuppressed { .. } => {}
        }
    }
}

fn age_seconds(since: DateTime<Utc>, observed_at: DateTime<Utc>) -> i64 {
    (observed_at - since).num_seconds().max(0)
}

fn count(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use auction_application::external_accounts::{
        ExternalAccountStatusReader, ExternalMaintenanceState,
    };
    use auction_application::external_metrics::{
        ExternalMetricsEvent, ExternalMetricsObserver, ForumOperation, ForumRequestCategory,
        ForumWorld, MailMaintenanceOutcome, MailOperation, MailRequestCategory, MailSignal,
        MailWorld,
    };

    use super::ExternalAccountTracker;

    #[test]
    fn fresh_tracker_reports_idle_without_invented_history() {
        let tracker = ExternalAccountTracker::default();
        let status = tracker.snapshot().expect("新 tracker 仍有合法快照");
        assert!(status.validate());
        assert!(!status.mail.busy);
        assert_eq!(status.mail.busy_age_seconds, None);
        assert_eq!(status.mail.request_count, 0);
        assert_eq!(status.mail.last_request_at, None);
        assert_eq!(status.forum.last_request_at, None);
        assert_eq!(status.maintenance.state, ExternalMaintenanceState::Idle);
        assert_eq!(status.maintenance.last_started_at, None);
    }

    #[test]
    fn mail_busy_tracks_account_busy_and_successful_recovery() {
        let tracker = ExternalAccountTracker::default();
        let observer: &dyn ExternalMetricsObserver = &tracker;
        observer.observe(ExternalMetricsEvent::MailRequestStarted {
            world: MailWorld::Main,
            operation: MailOperation::Inbox,
        });
        observer.observe(ExternalMetricsEvent::MailRequestStarted {
            world: MailWorld::Isekai,
            operation: MailOperation::Sent,
        });
        let status = tracker.snapshot().expect("请求快照");
        assert!(!status.mail.busy, "在途请求不等于远端账号忙碌");
        assert_eq!(status.mail.request_count, 2);
        assert!(status.mail.last_request_at.is_some());
        observer.observe(ExternalMetricsEvent::MailRequestSignal {
            world: MailWorld::Main,
            operation: MailOperation::Inbox,
            signal: MailSignal::AccountBusy,
        });
        let status = tracker.snapshot().expect("账号忙碌快照");
        assert!(status.mail.busy);
        assert_eq!(status.mail.busy_age_seconds, Some(0));

        observer.observe(ExternalMetricsEvent::MailRequestFinished {
            world: MailWorld::Main,
            operation: MailOperation::Inbox,
            category: MailRequestCategory::Ok,
        });
        let status = tracker.snapshot().expect("恢复快照");
        assert!(!status.mail.busy);
        assert_eq!(status.mail.busy_age_seconds, None);
        assert!(status.mail.last_success_at.is_some());
        observer.observe(ExternalMetricsEvent::MailRequestFinished {
            world: MailWorld::Isekai,
            operation: MailOperation::Sent,
            category: MailRequestCategory::AccountBusy,
        });
        let status = tracker.snapshot().expect("再次忙碌快照");
        assert!(status.mail.busy);
        assert!(status.mail.last_failure_at.is_some());
    }

    #[test]
    fn canceled_finish_is_neither_success_nor_failure() {
        let tracker = ExternalAccountTracker::default();
        let observer: &dyn ExternalMetricsObserver = &tracker;
        observer.observe(ExternalMetricsEvent::MailRequestFinished {
            world: MailWorld::Main,
            operation: MailOperation::Detail,
            category: MailRequestCategory::Canceled,
        });
        let status = tracker.snapshot().expect("取消快照");
        assert_eq!(status.mail.last_success_at, None);
        assert_eq!(status.mail.last_failure_at, None);
    }

    #[test]
    fn gate_waits_accumulate_all_outcomes() {
        let tracker = ExternalAccountTracker::default();
        let observer: &dyn ExternalMetricsObserver = &tracker;
        for (outcome, millis) in [
            (auction_application::external_metrics::WaitOutcome::Acquired, 1500_u64),
            (auction_application::external_metrics::WaitOutcome::Error, 2500_u64),
            (auction_application::external_metrics::WaitOutcome::Canceled, 900_u64),
        ] {
            observer.observe(ExternalMetricsEvent::MailGateWait {
                world: MailWorld::Main,
                operation: MailOperation::Send,
                outcome,
                waited: std::time::Duration::from_millis(millis),
            });
        }
        let status = tracker.snapshot().expect("等待快照");
        assert_eq!(status.mail.gate_wait_count, 3);
        assert_eq!(status.mail.gate_wait_seconds_total, 4);
    }

    #[test]
    fn forum_boundaries_and_write_unknown_are_separate_facts() {
        let tracker = ExternalAccountTracker::default();
        let observer: &dyn ExternalMetricsObserver = &tracker;
        observer.observe(ExternalMetricsEvent::ForumRequestStarted {
            world: ForumWorld::Shared,
            operation: ForumOperation::Profile,
        });
        observer.observe(ExternalMetricsEvent::ForumRequestFinished {
            world: ForumWorld::Shared,
            operation: ForumOperation::Profile,
            category: ForumRequestCategory::Ok,
        });
        observer.observe(ExternalMetricsEvent::ForumWriteUnknown {
            world: ForumWorld::Main,
            operation: ForumOperation::ReplyTopic,
        });
        let status = tracker.snapshot().expect("论坛快照");
        assert!(status.forum.last_request_at.is_some());
        assert!(status.forum.last_success_at.is_some());
        assert_eq!(status.forum.last_failure_at, None);
        assert_eq!(status.forum.write_unknown_count, 1);
    }

    #[test]
    fn maintenance_cycles_transition_through_running_and_finish() {
        let tracker = ExternalAccountTracker::default();
        let observer: &dyn ExternalMetricsObserver = &tracker;
        observer.observe(ExternalMetricsEvent::MailMaintenanceStarted { world: MailWorld::Main });
        let status = tracker.snapshot().expect("运行中维护");
        assert_eq!(status.maintenance.state, ExternalMaintenanceState::Running);
        assert_eq!(status.maintenance.running_age_seconds, Some(0));

        observer.observe(ExternalMetricsEvent::MailMaintenanceFinished {
            world: MailWorld::Main,
            outcome: MailMaintenanceOutcome::Succeeded,
        });
        let status = tracker.snapshot().expect("成功维护");
        assert_eq!(status.maintenance.state, ExternalMaintenanceState::Succeeded);
        assert_eq!(status.maintenance.running_age_seconds, None);
        assert!(status.maintenance.last_success_at.is_some());

        observer.observe(ExternalMetricsEvent::MailMaintenanceStarted { world: MailWorld::Isekai });
        observer.observe(ExternalMetricsEvent::MailMaintenanceFinished {
            world: MailWorld::Isekai,
            outcome: MailMaintenanceOutcome::Failed,
        });
        let status = tracker.snapshot().expect("失败维护");
        assert_eq!(status.maintenance.state, ExternalMaintenanceState::Failed);
        assert!(status.maintenance.last_failure_at.is_some());

        observer.observe(ExternalMetricsEvent::MailMaintenanceStarted { world: MailWorld::Main });
        observer.observe(ExternalMetricsEvent::MailMaintenanceFinished {
            world: MailWorld::Main,
            outcome: MailMaintenanceOutcome::Canceled,
        });
        let status = tracker.snapshot().expect("取消维护");
        assert_eq!(
            status.maintenance.state,
            ExternalMaintenanceState::Failed,
            "另一世界的未恢复失败不能被取消/成功清除"
        );
        assert!(status.maintenance.last_canceled_at.is_some());
        assert!(status.maintenance.last_failure_at.is_some());

        observer.observe(ExternalMetricsEvent::MailMaintenanceStarted { world: MailWorld::Isekai });
        observer.observe(ExternalMetricsEvent::MailMaintenanceFinished {
            world: MailWorld::Isekai,
            outcome: MailMaintenanceOutcome::Succeeded,
        });
        let status = tracker.snapshot().expect("失败世界已恢复");
        assert_eq!(status.maintenance.state, ExternalMaintenanceState::Succeeded);
        assert!(status.maintenance.last_failure_at.is_none());
    }

    #[test]
    fn main_failure_is_not_cleared_by_isekai_success() {
        let tracker = ExternalAccountTracker::default();
        let observer: &dyn ExternalMetricsObserver = &tracker;
        observer.observe(ExternalMetricsEvent::MailMaintenanceStarted { world: MailWorld::Main });
        observer.observe(ExternalMetricsEvent::MailMaintenanceFinished {
            world: MailWorld::Main,
            outcome: MailMaintenanceOutcome::Failed,
        });
        observer.observe(ExternalMetricsEvent::MailMaintenanceStarted { world: MailWorld::Isekai });
        observer.observe(ExternalMetricsEvent::MailMaintenanceFinished {
            world: MailWorld::Isekai,
            outcome: MailMaintenanceOutcome::Succeeded,
        });
        let status = tracker.snapshot().expect("跨世界维护快照");
        assert_eq!(status.maintenance.state, ExternalMaintenanceState::Failed);
        assert!(status.maintenance.last_failure_at.is_some());
    }

    #[test]
    fn running_world_keeps_snapshot_running_across_other_world_finish() {
        let tracker = ExternalAccountTracker::default();
        let observer: &dyn ExternalMetricsObserver = &tracker;
        observer.observe(ExternalMetricsEvent::MailMaintenanceStarted { world: MailWorld::Main });
        observer.observe(ExternalMetricsEvent::MailMaintenanceStarted { world: MailWorld::Isekai });
        observer.observe(ExternalMetricsEvent::MailMaintenanceFinished {
            world: MailWorld::Isekai,
            outcome: MailMaintenanceOutcome::Succeeded,
        });
        let status = tracker.snapshot().expect("仍有世界在运行");
        assert_eq!(status.maintenance.state, ExternalMaintenanceState::Running);
    }
}
