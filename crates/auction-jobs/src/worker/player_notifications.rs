//! 玩家通知扫描循环：卖方成交汇总与换季前 isekai 领取/交付提醒。
//!
//! 扫描只读持久事实并幂等入队；发送仍由普通 delivery 循环完成。循环失败只影响本 owner，
//! 不阻塞结算、履约或投递。

use std::sync::Arc;
use std::time::Duration;

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::core::{ApplicationError, Clock};
use auction_application::player_notifications::{
    PlayerNotificationError, PlayerNotificationService,
};
use auction_observability::{WorkerLoop, WorkerResult};

use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, poll_delay};

/// 单轮扫描与入队的总预算。超时取消是安全的：扫描只读，入队按稳定命令身份幂等，下一轮
/// 只会补齐尚未入队的通知；预算只防止一轮卡住时长期占用共享批量许可。
const SCAN_TIMEOUT: Duration = Duration::from_secs(60);

/// 扫描循环的调度参数；窗口与批量属于应用层常量，不随热配置改变。
#[derive(Debug, Clone, Copy)]
pub struct PlayerNotificationRuntimeConfig {
    pub poll_interval: Duration,
    pub jitter: Duration,
}

impl PlayerNotificationRuntimeConfig {
    fn validate(self) -> bool {
        !self.poll_interval.is_zero()
            && self.poll_interval <= Duration::from_secs(86_400)
            && self.jitter <= self.poll_interval
    }
}

/// 装配根注入的玩家通知运行时。
pub struct PlayerNotificationRuntime {
    pub service: Arc<PlayerNotificationService>,
    pub config: PlayerNotificationRuntimeConfig,
}

impl PlayerNotificationRuntime {
    pub fn new(
        service: Arc<PlayerNotificationService>,
        config: PlayerNotificationRuntimeConfig,
    ) -> Result<Self, ApplicationError> {
        if !config.validate() {
            return Err(ApplicationError::PlayerNotification(PlayerNotificationError::Unavailable));
        }
        Ok(Self { service, config })
    }
}

/// 按固定节奏扫描；扫描失败按 owner 记录后继续下一轮，不清空已入队通知。
pub(super) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    let Some(runtime) = worker.player_notifications.clone() else {
        return;
    };
    let diagnostic_metrics = worker.metrics.clone();
    // 与其他常驻循环一致：连接池指标、诊断字段与未持久化计数按本循环归属。
    let body = async move {
        loop {
            if worker.control.is_draining() {
                return;
            }
            let started = std::time::Instant::now();
            // 扫描属于批量轮次，受共享连接配额限制；等待下一轮期间不占用许可。
            let bulk_permit = worker.bulk_permit().await;
            let result = match tokio::time::timeout(SCAN_TIMEOUT, runtime.service.run_once()).await
            {
                Ok(Ok(_)) => WorkerResult::Success,
                Ok(Err(error)) => {
                    diagnostics::warn(
                        "player_notifications",
                        "scan",
                        error.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                    WorkerResult::Error
                }
                Err(elapsed) => {
                    diagnostics::unpersisted_error(
                        "player_notifications",
                        "scan_timeout",
                        diagnostics::timeout_kind(&elapsed),
                        DiagnosticContext::default(),
                    );
                    WorkerResult::Error
                }
            };
            worker.metrics.observe_iteration(
                WorkerLoop::PlayerNotifications,
                result,
                started.elapsed(),
            );
            drop(bulk_permit);

            let wait = poll_delay(runtime.config.poll_interval, runtime.config.jitter);
            let scheduled_wake = tokio::time::Instant::now() + wait;
            if crate::worker::runtime::wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
                return;
            }
            worker.metrics.observe_schedule_delay(
                WorkerLoop::PlayerNotifications,
                tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
            );
        }
    };
    diagnostics::scope(diagnostic_metrics, WorkerLoop::PlayerNotifications, body).await;
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use auction_application::auction::ApplicationFuture;
    use auction_application::core::diagnostics::SafeErrorKind;
    use auction_application::mail::{
        MailDeliveryActivation, MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryResult,
        MailNoticeEnqueue, MailNoticeLimit,
    };
    use auction_application::player_notifications::{
        PlayerNotificationScanner, PlayerNotificationService, SeasonReminderFact,
        SellerSummaryLotFact,
    };
    use auction_observability::{WorkerLoop, WorkerResult, current_database_worker_loop};
    use chrono::{DateTime, Utc};

    use super::{
        PlayerNotificationRuntime, PlayerNotificationRuntimeConfig, SCAN_TIMEOUT, run_loop,
    };
    use crate::worker::PersistentWorker;
    use crate::worker::diagnostics;
    use crate::worker::runtime::WorkerConnectionQuota;
    use crate::worker::runtime::test_fixture::{
        PausedClock, RecordingMetrics, base_time, unavailable, worker,
    };

    /// 记录调用时所在的循环归属，随后一直挂起，模拟卡在锁等待中的扫描。
    #[derive(Default)]
    struct StuckScanner {
        observed_loops: Mutex<Vec<Option<WorkerLoop>>>,
    }

    impl PlayerNotificationScanner for StuckScanner {
        fn seller_summary_facts(
            &self,
            _settled_since: DateTime<Utc>,
            _limit: u32,
        ) -> ApplicationFuture<'_, Vec<SellerSummaryLotFact>> {
            self.observed_loops.lock().unwrap().push(current_database_worker_loop());
            Box::pin(std::future::pending())
        }

        fn season_reminder_facts(
            &self,
            _now: DateTime<Utc>,
            _lead_seconds: i64,
            _limit: u32,
        ) -> ApplicationFuture<'_, Vec<SeasonReminderFact>> {
            unavailable()
        }
    }

    /// 扫描卡住时不会入队；被调用即失败关闭。
    struct InertEnqueuer;

    impl MailDeliveryEnqueuer for InertEnqueuer {
        fn enqueue(
            &self,
            _command: MailDeliveryCommand,
        ) -> ApplicationFuture<'_, MailDeliveryResult> {
            unavailable()
        }

        fn enqueue_within_limit(
            &self,
            _command: MailDeliveryCommand,
            _limit: MailNoticeLimit,
        ) -> ApplicationFuture<'_, MailNoticeEnqueue> {
            unavailable()
        }

        fn prepare_recovery(
            &self,
            _command: MailDeliveryCommand,
        ) -> ApplicationFuture<'_, MailDeliveryResult> {
            unavailable()
        }

        fn activate_recovery(
            &self,
            _activation: MailDeliveryActivation,
        ) -> ApplicationFuture<'_, MailDeliveryResult> {
            unavailable()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stuck_scan_times_out_inside_loop_scope_and_releases_permit() {
        // 未持久化计数经诊断 sink 按循环归属累计；生产由 Worker 运行时安装。
        diagnostics::install();
        let scanner = Arc::new(StuckScanner::default());
        let clock = Arc::new(PausedClock::new(base_time()));
        let service =
            PlayerNotificationService::new(scanner.clone(), Arc::new(InertEnqueuer), clock.clone());
        let runtime = PlayerNotificationRuntime::new(
            Arc::new(service),
            PlayerNotificationRuntimeConfig {
                poll_interval: Duration::from_secs(300),
                jitter: Duration::ZERO,
            },
        )
        .unwrap();
        let quota = Arc::new(WorkerConnectionQuota::new(1).unwrap());
        let metrics = Arc::new(RecordingMetrics::default());
        let worker = Arc::new(PersistentWorker {
            player_notifications: Some(Arc::new(runtime)),
            bulk_quota: Some(quota.clone()),
            metrics: metrics.clone(),
            ..worker(clock)
        });
        let (shutdown, receiver) = tokio::sync::watch::channel(false);
        let notifications = tokio::spawn(run_loop(worker, receiver));

        // 预算内扫描仍挂起，许可被本轮占用。
        tokio::time::sleep(SCAN_TIMEOUT - Duration::from_secs(1)).await;
        assert!(quota.try_acquire().is_none());
        assert!(metrics.iterations().is_empty());

        // 预算耗尽后本轮按 Error 收口，并在等待下一轮期间归还许可。
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            metrics.iterations(),
            vec![(WorkerLoop::PlayerNotifications, WorkerResult::Error)]
        );
        assert!(
            metrics
                .unpersisted()
                .contains(&(WorkerLoop::PlayerNotifications, SafeErrorKind::TIMEOUT)),
            "扫描超时必须按本循环计入未持久化错误"
        );
        assert!(quota.try_acquire().is_some(), "超时后不得继续占用批量许可");
        assert_eq!(
            *scanner.observed_loops.lock().unwrap(),
            vec![Some(WorkerLoop::PlayerNotifications)],
            "扫描必须在本循环的连接归属内执行"
        );

        shutdown.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), notifications).await.unwrap().unwrap();
    }
}
