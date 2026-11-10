// 玩家通知扫描循环：卖方成交汇总与换季前 isekai 领取/交付提醒。
//
// 扫描只读持久事实并幂等入队；发送仍由普通 delivery 循环完成。循环失败只影响本 owner，
// 不阻塞结算、履约或投递。

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
    loop {
        if worker.control.is_draining() {
            return;
        }
        let started = std::time::Instant::now();
        // 扫描属于批量轮次，受共享连接配额限制；等待下一轮期间不占用许可。
        let bulk_permit = worker.bulk_permit().await;
        let result = match runtime.service.run_once().await {
            Ok(_) => WorkerResult::Success,
            Err(error) => {
                diagnostics::warn(
                    "player_notifications",
                    "scan",
                    error.diagnostic_kind(),
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
}
