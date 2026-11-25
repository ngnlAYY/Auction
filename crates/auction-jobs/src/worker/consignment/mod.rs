//! 寄售 Worker 的启动门禁、就绪心跳与两个持久工作循环。

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, ListingAdmission, ListingLookupBudget,
    ListingMaintenanceService, LookupWorkStore, ResolveListingService,
};
use auction_application::core::Clock;
use auction_domain::valid_text;
use auction_observability::WorkerResult;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

mod collection;
mod deposits;
mod dispatch;
mod downstream_rounds;
mod heartbeat;
mod loop_kind;
mod maintenance;
mod order_recheck;
mod recovery;
mod redispatch;
mod resolve;
mod return_take;
mod rights_notice;
mod sanction;
mod seasons;
#[cfg(test)]
mod tests;

pub use super::mail::{QuarantineRecheckRuntime, run_quarantine};
pub use collection::{
    COLLECTION_PAGE_SIZE, CollectionObservation, CollectionObserver, CollectionRead,
    CollectionRound, CollectionRuntime, CollectionSource, run as run_collections,
};
pub use deposits::{DispositionMaintenanceRuntime, run as run_disposition_maintenance};
pub use dispatch::{
    ConsignmentDispatchRuntime, DispatchExecution, DispatchLease, DispatchRound, DispatchTransport,
    DispatchWork, DispatchWorkSource, PreparedDispatch, SentObservation, run as run_dispatch,
};
pub use downstream_rounds::{
    DownstreamRoundRuntime, DownstreamRoundSource, PendingDownstreamRound,
    run as run_downstream_rounds,
};
pub use heartbeat::MailEvidenceReadinessRuntime;
pub use loop_kind::ConsignmentLoop;
pub(crate) use loop_kind::observe_worker_iteration;
pub use order_recheck::{
    DispatchSentReader, OrderRecheckRuntime, RecheckRound, run as run_order_rechecks,
};
pub use recovery::{RecoveryRuntime, run as run_recovery};
pub use redispatch::{
    PendingRedispatch, RedispatchRuntime, RedispatchWorkSource, run as run_redispatch,
};
pub use return_take::{
    ClaimedTake, PreparedTake, ReturnTakeRuntime, ReturnTakeTransport, ReturnTakeWorkSource,
    TakeExecution, TakeProof, TakeRound, run as run_return_takes,
};
pub use rights_notice::{
    NoticeDelivery, RightNoticeDeliverer, RightNoticeRuntime, run as run_right_notices,
};
pub use sanction::{SanctionExecutionRuntime, run as run_sanction_execution};
pub use seasons::{SeasonCalendarRuntime, SeasonTaskRuntime, run as run_season_tasks};

/// 本次心跳检查的能力，检查失败或未知的项为 false。
#[derive(Clone, Copy, Debug, Default)]
pub struct ConsignmentDependencies {
    /// 当前数据库可达。
    pub database: bool,
    /// 当前数据库已具备所需 schema。
    pub schema: bool,
    /// 已装配可用的信封 opener。
    pub opener: bool,
    /// 已装配装备域当前 keyring。
    pub current_keyring: bool,
    /// 已装配 lookup。这一项只核装配，不访问第三方。
    pub lookup: bool,
}

/// 装配根提供的寄售依赖就绪检查。每次请求心跳时重新检查。
pub trait ConsignmentReadinessProbe: Send + Sync {
    /// 返回本次能力观察；任何无法确认的能力均不得标为可用。
    fn dependencies(&self) -> ConsignmentFuture<'_, ConsignmentDependencies>;
}

/// 启用时所需的解析运行参数。用例须使用同一 Worker 的外部访问暂停快照。
pub struct ConsignmentLookupRuntime {
    /// 应用层单轮解析用例。
    pub service: ResolveListingService,
    /// 用于回收和续发心跳的工作仓储，与解析用例注入同一实例。
    pub work: Arc<dyn LookupWorkStore>,
    /// 每次领取的解析和全局间隔预算。
    pub budget: ListingLookupBudget,
    /// 本地唤醒时钟，仅把用例给出的绝对时刻换算成等待。
    pub clock: Arc<dyn Clock>,
    /// 当前 Worker 的租约实例名。
    pub worker_id: String,
    /// 可选通知生产者使用 notify_one 合并唤醒；未接生产者时保留有界轮询。
    pub wakeup: Arc<Notify>,
    /// 当前 Worker 使用的寄售依赖检查。
    pub readiness: Arc<dyn ConsignmentReadinessProbe>,
}

/// 已同步持久准入、由装配根交给单个受监管任务的寄售 Worker。
pub struct ConsignmentWorker {
    admission: ListingAdmission,
    maintenance: ListingMaintenanceService,
    lookup: Option<ConsignmentLookupRuntime>,
    mail_readiness: Vec<MailEvidenceReadinessRuntime>,
    disposition: Option<Arc<DispositionMaintenanceRuntime>>,
    quarantine: Option<Arc<QuarantineRecheckRuntime>>,
    seasons: Option<Arc<SeasonTaskRuntime>>,
    sanctions: Option<Arc<SanctionExecutionRuntime>>,
    right_notices: Option<Arc<RightNoticeRuntime>>,
    recovery: Option<Arc<RecoveryRuntime>>,
    dispatch: Option<Arc<ConsignmentDispatchRuntime>>,
    redispatch: Option<Arc<RedispatchRuntime>>,
    downstream_rounds: Option<Arc<DownstreamRoundRuntime>>,
    return_takes: Option<Arc<ReturnTakeRuntime>>,
    order_rechecks: Option<Arc<OrderRecheckRuntime>>,
    collections: Option<Arc<CollectionRuntime>>,
}

impl ConsignmentWorker {
    /// 监听或宣告就绪前调用。关闭时仅当存量探针证实有清退工作才返回维护任务。
    pub async fn prepare(
        enabled: bool,
        artifact_capable: bool,
        maintenance: ListingMaintenanceService,
        lookup: Option<ConsignmentLookupRuntime>,
        closed_has_stock: impl Future<Output = Result<bool, ConsignmentError>>,
    ) -> Result<Option<Self>, ConsignmentError> {
        // 由用例拒绝制品不支持的启用请求，无需先确认解析依赖。
        let lookup = if enabled && artifact_capable {
            Some(
                lookup
                    .filter(|runtime| valid_text(&runtime.worker_id, 256))
                    .ok_or(ConsignmentError::Unavailable)?,
            )
        } else {
            None
        };
        // 不变量 2、5、6：持久代次同步成功前不得构造可运行的任务。
        let admission = maintenance.sync_admission(enabled, artifact_capable).await?;
        // 同步回执没有存量字段；由数据库探针确认，读取失败不能视为空库。
        if !admission.admitting && !closed_has_stock.await? {
            return Ok(None);
        }
        Ok(Some(Self {
            admission,
            maintenance,
            lookup,
            mail_readiness: Vec::new(),
            disposition: None,
            quarantine: None,
            seasons: None,
            sanctions: None,
            right_notices: None,
            recovery: None,
            dispatch: None,
            redispatch: None,
            downstream_rounds: None,
            return_takes: None,
            order_rechecks: None,
            collections: None,
        }))
    }

    /// 注入已装配且就绪的收件读取器；未注入的世界不会写收件就绪。
    pub fn with_mail_readiness(
        mut self,
        mail_readiness: Vec<MailEvidenceReadinessRuntime>,
    ) -> Self {
        self.mail_readiness = mail_readiness;
        self
    }

    /// 注入处置维护运行时；关闭寄售但仍有存量时也会运行。
    pub fn with_disposition(mut self, disposition: Arc<DispositionMaintenanceRuntime>) -> Self {
        self.disposition = Some(disposition);
        self
    }

    /// 注入隔离重检运行时；关闭寄售但仍有存量时也会运行。
    pub fn with_quarantine(mut self, quarantine: Arc<QuarantineRecheckRuntime>) -> Self {
        self.quarantine = Some(quarantine);
        self
    }

    /// 注入季任务运行时；关闭寄售但仍有存量时也会运行。
    pub fn with_seasons(mut self, seasons: Arc<SeasonTaskRuntime>) -> Self {
        self.seasons = Some(seasons);
        self
    }

    /// 注入处罚执行运行时；关闭寄售但仍有存量时也会运行，存量处罚必须收口。
    pub fn with_sanctions(mut self, sanctions: Arc<SanctionExecutionRuntime>) -> Self {
        self.sanctions = Some(sanctions);
        self
    }

    /// 注入权利通知运行时；只在持久准入同步为开放时运行，关闭寄售不再对外发码。
    pub fn with_right_notices(mut self, right_notices: Arc<RightNoticeRuntime>) -> Self {
        self.right_notices = Some(right_notices);
        self
    }

    /// 注入恢复处置运行时（退回处置、自动重列、付款提醒）；关闭寄售但仍有存量时也会运行，
    /// 已接受的订单必须收口。
    pub fn with_recovery(mut self, recovery: Arc<RecoveryRuntime>) -> Self {
        self.recovery = Some(recovery);
        self
    }

    /// 注入派发 executor；关闭寄售但仍有存量时也会运行：已成交订单的首派与补寄是续办旧权利（D9 §355），
    /// 新购买已在购买入口被关闭截止。
    pub fn with_dispatch(mut self, dispatch: Arc<ConsignmentDispatchRuntime>) -> Self {
        self.dispatch = Some(dispatch);
        self
    }

    /// 注入补寄消费端；关闭寄售但仍有存量时也会运行，已排队的补寄必须收口。
    pub fn with_redispatch(mut self, redispatch: Arc<RedispatchRuntime>) -> Self {
        self.redispatch = Some(redispatch);
        self
    }

    /// 注入下游首轮消费端；关闭寄售但仍有存量时也会运行，已成交订单的交割、卖款与退款是续办旧权利。
    pub fn with_downstream_rounds(
        mut self,
        downstream_rounds: Arc<DownstreamRoundRuntime>,
    ) -> Self {
        self.downstream_rounds = Some(downstream_rounds);
        self
    }

    /// 注入退件 Take executor；关闭寄售但仍有存量时也会运行，已接管的退件必须取回。
    pub fn with_return_takes(mut self, return_takes: Arc<ReturnTakeRuntime>) -> Self {
        self.return_takes = Some(return_takes);
        self
    }

    /// 注入订单复核消费端；关闭寄售但仍有存量时也会运行，Admin 已排队的复核必须收口。
    pub fn with_order_rechecks(mut self, order_rechecks: Arc<OrderRecheckRuntime>) -> Self {
        self.order_rechecks = Some(order_rechecks);
        self
    }

    /// 注入收款与确领推进循环；关闭寄售但仍有存量时也会运行，已成交订单的收款、交割与卖款是续办旧权利。
    pub fn with_collections(mut self, collections: Arc<CollectionRuntime>) -> Self {
        self.collections = Some(collections);
        self
    }

    /// 在现有 Worker 任务组或任务守卫下运行。发送 drain 后等待本任务，沿用进程共享排空期限。
    /// 指标回调依次接收循环、结果、耗时和本页清退失败数。非维护页迭代的失败数为零。
    pub async fn run(
        self,
        drain: watch::Receiver<bool>,
        observe: impl Fn(ConsignmentLoop, WorkerResult, Duration, u32) + Send + Sync,
    ) {
        let Self {
            admission,
            maintenance,
            lookup,
            mail_readiness,
            disposition,
            quarantine,
            seasons,
            sanctions,
            right_notices,
            recovery,
            dispatch,
            redispatch,
            downstream_rounds,
            return_takes,
            order_rechecks,
            collections,
        } = self;
        let epoch = admission.service_epoch;
        // 权利通知按持久准入开关：关闭寄售后不再向外发码（未发的码随挑战过期在领取时被抑制）。
        let right_notices = right_notices.filter(|_| admission.admitting);
        let work = lookup.as_ref().map(|runtime| runtime.work.as_ref());
        let maintain = maintenance::run(&maintenance, work, drain.clone(), &observe);
        let resolve_lookup = async {
            if let Some(lookup) = &lookup {
                let observe_lookup = |kind, result, duration| observe(kind, result, duration, 0);
                resolve::run(lookup, epoch, drain.clone(), &observe_lookup).await;
            }
        };
        // 收件就绪心跳只被 poll 门控读取，而 poll 门控本身要求寄售已激活；寄售关闭时
        // 不续期不会阻塞处置与重检循环（它们按持久存量独立领取），因此这里随 lookup
        // 装配一起停止即可。
        let heartbeat_mail = async {
            if let Some(lookup) = &lookup {
                let observe_lookup = |kind, result, duration| observe(kind, result, duration, 0);
                heartbeat::run(lookup, &mail_readiness, epoch, drain.clone(), &observe_lookup)
                    .await;
            }
        };
        let disposition_loop = async {
            if let Some(runtime) = disposition {
                deposits::run(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::Disposition, result, duration, 0);
                })
                .await;
            }
        };
        let quarantine_loop = async {
            if let Some(runtime) = quarantine {
                run_quarantine(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::Quarantine, result, duration, 0);
                })
                .await;
            }
        };
        let seasons_loop = async {
            if let Some(runtime) = seasons {
                run_season_tasks(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::Seasons, result, duration, 0);
                })
                .await;
            }
        };
        let sanctions_loop = async {
            if let Some(runtime) = sanctions {
                run_sanction_execution(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::Sanctions, result, duration, 0);
                })
                .await;
            }
        };
        let right_notices_loop = async {
            if let Some(runtime) = right_notices {
                run_right_notices(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::RightNotices, result, duration, 0);
                })
                .await;
            }
        };
        let recovery_loop = async {
            if let Some(runtime) = recovery {
                run_recovery(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::Recovery, result, duration, 0);
                })
                .await;
            }
        };
        let dispatch_loop = async {
            if let Some(runtime) = dispatch {
                run_dispatch(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::Dispatch, result, duration, 0);
                })
                .await;
            }
        };
        let redispatch_loop = async {
            if let Some(runtime) = redispatch {
                run_redispatch(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::Redispatch, result, duration, 0);
                })
                .await;
            }
        };
        let downstream_rounds_loop = async {
            if let Some(runtime) = downstream_rounds {
                run_downstream_rounds(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::DownstreamRounds, result, duration, 0);
                })
                .await;
            }
        };
        let return_takes_loop = async {
            if let Some(runtime) = return_takes {
                run_return_takes(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::ReturnTakes, result, duration, 0);
                })
                .await;
            }
        };
        let order_rechecks_loop = async {
            if let Some(runtime) = order_rechecks {
                run_order_rechecks(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::OrderRechecks, result, duration, 0);
                })
                .await;
            }
        };
        let collections_loop = async {
            if let Some(runtime) = collections {
                run_collections(runtime, drain.clone(), |result, duration| {
                    observe(ConsignmentLoop::Collections, result, duration, 0);
                })
                .await;
            }
        };
        tokio::join!(
            maintain,
            resolve_lookup,
            heartbeat_mail,
            disposition_loop,
            quarantine_loop,
            seasons_loop,
            sanctions_loop,
            right_notices_loop,
            recovery_loop,
            dispatch_loop,
            redispatch_loop,
            downstream_rounds_loop,
            return_takes_loop,
            order_rechecks_loop,
            collections_loop
        );
    }
}

fn stopped(drain: &watch::Receiver<bool>) -> bool {
    *drain.borrow() || drain.has_changed().is_err()
}

async fn wait_for_stop(mut drain: watch::Receiver<bool>) {
    // 通道关闭也表示 owner 已离开，不能继续领取。
    let _drain_or_owner_gone = drain.wait_for(|stopped| *stopped).await;
}

fn observe_iteration(
    observe: &impl Fn(ConsignmentLoop, WorkerResult, Duration),
    loop_kind: ConsignmentLoop,
    result: WorkerResult,
    started_at: Instant,
) {
    observe(loop_kind, result, started_at.elapsed());
}

fn error_code(error: ConsignmentError) -> &'static str {
    match error {
        ConsignmentError::LeaseLost => "lease_lost",
        ConsignmentError::ConditionNotMet => "condition_not_met",
        other => other.api_error().map_or("consignment_unavailable", |(_, code)| code),
    }
}
