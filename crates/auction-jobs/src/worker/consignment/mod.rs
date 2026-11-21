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

mod heartbeat;
mod maintenance;
mod resolve;
#[cfg(test)]
mod tests;

/// 供指标装配映射到 Worker 固定循环标签的闭集。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsignmentLoop {
    /// 单并发装备解析。
    Resolve,
    /// 本地补扫与租约回收。
    Maintenance,
    /// 持久准入就绪心跳。
    Readiness,
}

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
}

impl ConsignmentWorker {
    /// 监听或宣告就绪前调用。关闭时仅当存量探针证实有清退工作才返回维护任务。
    #[allow(dead_code, reason = "持久 Worker 装配由集成人接入")]
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
        Ok(Some(Self { admission, maintenance, lookup }))
    }

    /// 在现有 Worker 任务组或任务守卫下运行。发送 drain 后等待本任务，沿用进程共享排空期限。
    /// 指标回调依次接收循环、结果、耗时和本页清退失败数。非维护页迭代的失败数为零。
    #[allow(dead_code, reason = "持久 Worker 任务注册由集成人接入")]
    pub async fn run(
        self,
        drain: watch::Receiver<bool>,
        observe: impl Fn(ConsignmentLoop, WorkerResult, Duration, u32) + Send + Sync,
    ) {
        let epoch = self.admission.service_epoch;
        let work = self.lookup.as_ref().map(|runtime| runtime.work.as_ref());
        let maintain = maintenance::run(&self.maintenance, work, drain.clone(), &observe);
        if let Some(lookup) = &self.lookup {
            let observe_lookup = |kind, result, duration| observe(kind, result, duration, 0);
            tokio::join!(
                maintain,
                resolve::run(lookup, epoch, drain.clone(), &observe_lookup),
                heartbeat::run(lookup, epoch, drain, &observe_lookup),
            );
        } else {
            maintain.await;
        }
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
