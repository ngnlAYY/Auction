//! 按数据库时刻续发寄售准入就绪心跳。

use std::time::Duration;

use auction_application::consignment::ListingWorkerReadiness;
use auction_observability::WorkerResult;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};

use super::ConsignmentLookupRuntime;
use super::ConsignmentLoop;
use super::{error_code, observe_iteration, stopped, wait_for_stop};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

pub(super) async fn run(
    runtime: &ConsignmentLookupRuntime,
    epoch: u64,
    drain: watch::Receiver<bool>,
    observe: &(impl Fn(ConsignmentLoop, WorkerResult, Duration) + Sync),
) {
    let mut ticks = tokio::time::interval(HEARTBEAT_INTERVAL);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    while !stopped(&drain) {
        let started_at = tokio::select! {
            biased;
            () = wait_for_stop(drain.clone()) => return,
            _ = ticks.tick() => Instant::now(),
        };
        let dependencies = tokio::select! {
            biased;
            () = wait_for_stop(drain.clone()) => return,
            result = runtime.readiness.dependencies() => result,
        };
        if stopped(&drain) {
            return;
        }
        // 不变量 5、6：任一能力未就绪时保留原期限，续期的时钟、代次和写入由数据库裁定。
        let result = match dependencies {
            Ok(deps)
                if deps.database
                    && deps.schema
                    && deps.opener
                    && deps.current_keyring
                    && deps.lookup =>
            {
                runtime.work.record_worker_readiness(epoch, ListingWorkerReadiness::Ready).await
            }
            Ok(_) => {
                tracing::debug!(code = "dependencies_unavailable", "寄售本轮不续就绪心跳");
                observe_iteration(
                    observe,
                    ConsignmentLoop::Readiness,
                    WorkerResult::Error,
                    started_at,
                );
                continue;
            }
            Err(error) => Err(error),
        };
        let outcome = match result {
            Ok(()) => WorkerResult::Success,
            Err(error) => {
                tracing::warn!(code = error_code(error), "寄售就绪心跳未确认续期");
                WorkerResult::Error
            }
        };
        observe_iteration(observe, ConsignmentLoop::Readiness, outcome, started_at);
    }
}
