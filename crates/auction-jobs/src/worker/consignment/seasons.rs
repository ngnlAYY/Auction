//! 寄售季任务循环：冻结季窗、排队季前通知、建立 Send 投递并在季末关闭旧季。
//!
//! 所有决定都读持久季窗、通知、position 与挂单；内存定时器只负责唤醒。单轮的冻结失败
//! 只记结构化警告并继续处理已冻结季窗，不退出 Worker。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, DepositCommandService, SeasonCloseTarget, SeasonNoticeKind, SeasonTaskStore,
    season_notice_delivery_command,
};
use auction_application::core::Clock;
use auction_domain::SettlementWorld;
use auction_domain::consignment::{ConsignmentCalendar, FrozenSeason};
use auction_observability::WorkerResult;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

const IDLE_POLL: Duration = Duration::from_secs(60);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);

/// 季窗计算所需的日历与持久化时区。
pub struct SeasonCalendarRuntime {
    /// 与换季提醒同源的早鸟日历。
    pub calendar: ConsignmentCalendar,
    /// IANA 时区名，只写入季窗行。
    pub timezone: String,
}

/// 季任务运行参数。
pub struct SeasonTaskRuntime {
    /// 季窗、通知与投递的持久端口。
    pub tasks: Arc<dyn SeasonTaskStore>,
    /// 同一寄售命令服务。
    pub commands: Arc<DepositCommandService>,
    /// 寄售激活且日历可用时冻结当前季；缺失时只处理存量。
    pub calendar: Option<SeasonCalendarRuntime>,
    /// 配置的提前本地日历日数。
    pub preclose_days: u8,
    /// 是否允许冻结新季窗；关闭寄售时只处理存量。
    pub active: bool,
    /// 只把用例给出的绝对时刻换算成等待。
    pub clock: Arc<dyn Clock>,
    /// 可选唤醒；丢失时由有界轮询恢复。
    pub wakeup: Arc<Notify>,
}

/// 单轮是否产生了新的持久工作或错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Round {
    worked: bool,
    failed: bool,
}

impl SeasonTaskRuntime {
    /// 执行一轮 a–d；单项失败记警告并继续其他季窗。
    async fn round(&self) -> Round {
        let mut round = Round { worked: false, failed: false };
        if self.active
            && let Some(calendar) = &self.calendar
        {
            match calendar.calendar.season_window(self.clock.now(), self.preclose_days) {
                Ok(season) => {
                    match self.tasks.freeze_season_window(season, &calendar.timezone).await {
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(
                                code = error_code(error),
                                "寄售季窗冻结失败，保留已冻结界并继续"
                            );
                            round.failed = true;
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "寄售季窗无法从日历计算，本轮不冻结");
                }
            }
        }

        let seasons = match self.tasks.frozen_seasons().await {
            Ok(seasons) => seasons,
            Err(error) => {
                tracing::warn!(code = error_code(error), "读取已冻结寄售季窗失败");
                return Round { worked: false, failed: true };
            }
        };
        for season in seasons {
            match self.queue_notices(&season).await {
                Ok(worked) => round.worked |= worked,
                Err(error) => {
                    tracing::warn!(code = error_code(error), "寄售季前通知排队失败");
                    round.failed = true;
                }
            }
            match self.create_deliveries(&season).await {
                Ok(worked) => round.worked |= worked,
                Err(error) => {
                    tracing::warn!(code = error_code(error), "寄售季前通知投递建立失败");
                    round.failed = true;
                }
            }
            match self.close_if_due(&season).await {
                Ok(worked) => round.worked |= worked,
                Err(error) => {
                    tracing::warn!(code = error_code(error), "寄售季末关闭失败");
                    round.failed = true;
                }
            }
        }
        round
    }

    async fn queue_notices(&self, season: &FrozenSeason) -> Result<bool, ConsignmentError> {
        let targets = self.tasks.season_notice_targets(season).await?;
        let mut worked = false;
        for target in targets {
            match self
                .commands
                .queue_season_notice(
                    season.clone(),
                    target.position_id,
                    target.seller_user_id,
                    SeasonNoticeKind::PrecloseWithdrawal,
                )
                .await
            {
                Ok(_) => worked = true,
                Err(error) => {
                    tracing::warn!(code = error_code(error), "单个寄售季前通知排队失败");
                }
            }
        }
        Ok(worked)
    }

    async fn create_deliveries(&self, season: &FrozenSeason) -> Result<bool, ConsignmentError> {
        let deliveries = self.tasks.season_notice_deliveries(season).await?;
        let mut worked = false;
        for delivery in deliveries {
            let command = match season_notice_delivery_command(
                season,
                delivery.notice_id,
                &delivery.seller_name,
            ) {
                Ok(command) => command,
                Err(error) => {
                    tracing::warn!(code = error_code(error), "寄售季前通知无法构造投递命令");
                    continue;
                }
            };
            if let Err(error) = self.tasks.create_season_notice_delivery(command).await {
                tracing::warn!(code = error_code(error), "寄售季前通知投递建立失败");
                continue;
            }
            worked = true;
        }
        Ok(worked)
    }

    async fn close_if_due(&self, season: &FrozenSeason) -> Result<bool, ConsignmentError> {
        if !self.tasks.season_close_pending(season).await? {
            return Ok(false);
        }
        self.commands
            .close_season(season.clone(), SeasonCloseTarget::World(SettlementWorld::Isekai))
            .await?;
        Ok(true)
    }
}

/// 运行季任务循环，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<SeasonTaskRuntime>,
    mut drain: watch::Receiver<bool>,
    observe: impl Fn(WorkerResult, Duration) + Send + Sync,
) {
    let mut backoff = BACKOFF_INITIAL;
    while !stopped(&drain) {
        let started_at = Instant::now();
        let round = runtime.round().await;
        observe(
            if round.failed { WorkerResult::Error } else { WorkerResult::Success },
            started_at.elapsed(),
        );
        if round.failed {
            if wait_for_backoff(&mut drain, backoff).await {
                return;
            }
            backoff = backoff.saturating_mul(2).min(BACKOFF_MAXIMUM);
        } else if round.worked {
            backoff = BACKOFF_INITIAL;
            tokio::task::yield_now().await;
        } else {
            backoff = BACKOFF_INITIAL;
            if wait_for_wake(&mut drain, runtime.wakeup.as_ref(), IDLE_POLL).await {
                return;
            }
        }
    }
}

fn stopped(drain: &watch::Receiver<bool>) -> bool {
    *drain.borrow() || drain.has_changed().is_err()
}

async fn wait_for_wake(
    drain: &mut watch::Receiver<bool>,
    wakeup: &Notify,
    delay: Duration,
) -> bool {
    tokio::select! {
        biased;
        changed = drain.changed() => changed.is_err() || *drain.borrow(),
        () = wakeup.notified() => false,
        () = tokio::time::sleep(delay) => false,
    }
}

async fn wait_for_backoff(drain: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        biased;
        changed = drain.changed() => changed.is_err() || *drain.borrow(),
        () = tokio::time::sleep(delay) => false,
    }
}

fn error_code(error: ConsignmentError) -> &'static str {
    error.api_error().map_or("consignment_unavailable", |(_, code)| code)
}
