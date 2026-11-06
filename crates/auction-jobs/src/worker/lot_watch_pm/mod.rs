//! 独立 PM 调度；共享论坛网关，delivery cadence，每轮最多一次发送及一次只读核对。
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::worker::diagnostics;

mod delivery;

use auction_application::forum::ForumPmSender;
use auction_application::lot_watch_pm::{LotWatchPmClaimRequest, LotWatchPmWorkStore};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

use super::runtime::wait_until_or_shutdown;

/// 不依赖 MM 开关；总工作预算严格短于论坛租约，不并发批量发送。
#[derive(Clone)]
pub struct LotWatchPmRuntime {
    store: Arc<dyn LotWatchPmWorkStore>,
    sender: Arc<dyn ForumPmSender>,
    poll_interval: Duration,
    jitter: Duration,
    item_timeout: Duration,
    lease_seconds: i64,
    metrics: Arc<dyn auction_observability::WorkerRecorder>,
    external_metrics: Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
    /// W14.8 通知阶段指标；PM 渠道没有 outbox，只在终态与退役处记录。
    notifications: Arc<dyn auction_application::notification_metrics::NotificationRecorder>,
    write_lifecycle: auction_application::operations::ExternalWriteLifecycle,
}

impl LotWatchPmRuntime {
    /// 复用 delivery 轮询与 forum 总超时/租约；无需新增配置开关。
    pub fn new(
        store: Arc<dyn LotWatchPmWorkStore>,
        sender: Arc<dyn ForumPmSender>,
        poll_interval: Duration,
        jitter: Duration,
        item_timeout: Duration,
        lease: Duration,
    ) -> Result<Self, &'static str> {
        // 持久化接口只接受整秒；比较实际领取时长，不能让小数秒超出数据库 lease。
        let lease_seconds = lease.as_secs();
        if poll_interval.is_zero()
            || item_timeout.is_zero()
            || lease_seconds == 0
            || lease_seconds > 86400
            || item_timeout >= Duration::from_secs(lease_seconds)
        {
            return Err("PM 轮询、总预算与论坛租约无效");
        }
        Ok(Self {
            store,
            sender,
            poll_interval,
            jitter,
            item_timeout,
            lease_seconds: lease_seconds as i64,
            metrics: auction_observability::noop_worker_recorder(),
            external_metrics: auction_application::external_metrics::noop_external_metrics_observer(
            ),
            notifications: auction_application::notification_metrics::noop_notification_recorder(),
            write_lifecycle: auction_application::operations::ExternalWriteLifecycle::default(),
        })
    }

    /// 注入进程内活动守卫；零预算旧路径不注入。
    pub fn with_write_lifecycle(
        mut self,
        lifecycle: auction_application::operations::ExternalWriteLifecycle,
    ) -> Self {
        self.write_lifecycle = lifecycle;
        self
    }

    /// 注入 W14.8 通知指标；关闭指标时为 no-op，不改变发送与对账语义。
    pub fn with_notifications(
        mut self,
        recorder: Arc<dyn auction_application::notification_metrics::NotificationRecorder>,
    ) -> Self {
        self.notifications = recorder;
        self
    }

    /// 注入同步外部客户端指标；关闭 metrics 时为 no-op。
    pub fn with_external_metrics(
        mut self,
        observer: Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
    ) -> Self {
        self.external_metrics = observer;
        self
    }

    /// 记录 PM 单项总预算耗尽；只读同步计数，不改变持久 fence。
    pub(crate) fn observe_budget(
        &self,
        world: auction_domain::SettlementWorld,
        phase: auction_application::external_metrics::ForumBudgetPhase,
    ) {
        let world = match world {
            auction_domain::SettlementWorld::Main => {
                auction_application::external_metrics::ForumWorld::Main
            }
            auction_domain::SettlementWorld::Isekai => {
                auction_application::external_metrics::ForumWorld::Isekai
            }
        };
        self.external_metrics.observe(
            auction_application::external_metrics::ExternalMetricsEvent::ForumBudgetExhausted {
                world,
                phase,
            },
        );
    }

    /// 注入同一进程 recorder；PM 使用 Forum 闭集，不新增标签。
    pub fn with_metrics(mut self, metrics: Arc<dyn auction_observability::WorkerRecorder>) -> Self {
        self.metrics = metrics;
        diagnostics::install();
        self
    }

    /// drain 停止领取；在途工作仅等待到总预算，取消后依赖持久 fence 恢复。
    pub async fn run(self, worker_id: String, mut drain: watch::Receiver<bool>) {
        let diagnostic_metrics = self.metrics.clone();
        let diagnostic_loop = auction_observability::WorkerLoop::Forum;
        let body = async move {
            loop {
                if *drain.borrow() {
                    break;
                }
                for reconcile in [false, true] {
                    if *drain.borrow() {
                        break;
                    }
                    let available_worlds = [
                        auction_domain::SettlementWorld::Main,
                        auction_domain::SettlementWorld::Isekai,
                    ]
                    .into_iter()
                    .filter(|world| self.sender.has_available_forum_credential(*world))
                    .collect();
                    let request = LotWatchPmClaimRequest {
                        worker_id: worker_id.clone(),
                        lease_seconds: self.lease_seconds,
                        reconcile,
                        available_worlds,
                    };
                    let deadline = tokio::time::Instant::now() + self.item_timeout;
                    let claimed_world = std::sync::Arc::new(std::sync::Mutex::new(None));
                    let activity = self
                        .write_lifecycle
                        .activity(auction_application::operations::CriticalWriteKind::ForumPm);
                    let operation = tokio::time::timeout_at(
                        deadline,
                        delivery::process(
                            &self,
                            request,
                            &drain,
                            deadline,
                            &activity,
                            &claimed_world,
                        ),
                    );
                    let Some(result) = activity.run(operation).await else {
                        return;
                    };
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            diagnostics::unpersisted_error(
                                "forum_pm",
                                "deliver_notifications",
                                error.diagnostic_kind(),
                                DiagnosticContext::default(),
                            );
                        }
                        Err(error) => {
                            let world = match claimed_world.lock() {
                                Ok(guard) => *guard,
                                Err(poisoned) => *poisoned.into_inner(),
                            };
                            if let Some(world) = world {
                                self.observe_budget(
                                    world,
                                    auction_application::external_metrics::ForumBudgetPhase::PmItem,
                                );
                            }
                            diagnostics::unpersisted_error(
                                "forum_pm",
                                "deliver_notifications_timeout",
                                diagnostics::timeout_kind(&error),
                                DiagnosticContext::default(),
                            );
                        }
                    }
                }
                // 保留 PM 既有毫秒离散、上界不含 max 的抖动；仅复用停机等待门禁。
                let max = self.jitter.as_millis().min(u128::from(u64::MAX)) as u64;
                let jitter = if max == 0 {
                    0
                } else {
                    (uuid::Uuid::new_v4().as_u128() % u128::from(max)) as u64
                };
                let deadline = tokio::time::Instant::now()
                    + self.poll_interval.saturating_add(Duration::from_millis(jitter));
                if wait_until_or_shutdown(&mut drain, deadline).await {
                    break;
                }
            }
        };
        diagnostics::scope(diagnostic_metrics, diagnostic_loop, body).await
    }
}
