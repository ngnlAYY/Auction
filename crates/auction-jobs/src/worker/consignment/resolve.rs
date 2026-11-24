//! 单并发解析循环，根据应用用例结果决定下一次唤醒时刻。

use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingLookupRound, LookupClaimRequest,
};
use auction_observability::WorkerResult;
use tokio::sync::watch;
use tokio::time::Instant;
use uuid::Uuid;

use super::ConsignmentLookupRuntime;
use super::ConsignmentLoop;
use super::{error_code, observe_iteration, stopped, wait_for_stop};
use crate::worker::runtime::{wait_until_or_shutdown, wait_while_paused};

const IDLE_POLL: Duration = Duration::from_secs(5);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);

pub(super) async fn run(
    runtime: &ConsignmentLookupRuntime,
    epoch: u64,
    mut drain: watch::Receiver<bool>,
    observe: &(impl Fn(ConsignmentLoop, WorkerResult, Duration) + Sync),
) {
    let mut backoff = BACKOFF_INITIAL;
    while !stopped(&drain) {
        let started_at = Instant::now();
        let result = match LookupClaimRequest::new(
            epoch,
            runtime.worker_id.clone(),
            Uuid::new_v4(),
            runtime.budget,
        ) {
            Ok(request) => {
                // 不变量 5、6：只传递停止信号，保留用例 future 直到提交或按原围栏让出。
                // 超过进程的共享排空期限才由上层守卫中止，原持久时间栅栏仍然有效。
                runtime.service.run_once(request, wait_for_stop(drain.clone())).await
            }
            Err(error) => Err(error),
        };

        let outcome = match result {
            Ok(ListingLookupRound::Cancelled | ListingLookupRound::LeaseLost)
            | Err(ConsignmentError::LeaseLost) => WorkerResult::Canceled,
            Err(ConsignmentError::Business(ListingBusinessRejection::ListingNotActive)) => {
                // 维护者裁定：解析提交遇强制终止，挂单和拒绝回执同事务收口。
                WorkerResult::Success
            }
            Ok(_) => WorkerResult::Success,
            Err(error) => {
                tracing::warn!(code = error_code(error), "寄售解析一轮未确认完成");
                WorkerResult::Error
            }
        };
        observe_iteration(observe, ConsignmentLoop::Resolve, outcome, started_at);
        if stopped(&drain) || matches!(result, Ok(ListingLookupRound::Cancelled)) {
            return;
        }
        if outcome == WorkerResult::Error {
            if wait_until_or_shutdown(&mut drain, Instant::now() + backoff).await {
                return;
            }
            backoff = backoff.saturating_mul(2).min(BACKOFF_MAXIMUM);
            continue;
        }
        backoff = BACKOFF_INITIAL;

        match result {
            Ok(ListingLookupRound::Idle { wake_at }) => {
                // 不变量 5：过去的提示无法缩短轮询；时钟差或无提示时仍有界唤醒。
                let delay = wake_at
                    .and_then(|wake_at| (wake_at - runtime.clock.now()).to_std().ok())
                    .filter(|delay| !delay.is_zero())
                    .map_or(IDLE_POLL, |delay| delay.min(IDLE_POLL));
                tokio::select! {
                    biased;
                    () = wait_for_stop(drain.clone()) => return,
                    () = runtime.wakeup.notified() => {}
                    () = tokio::time::sleep(delay) => {}
                }
            }
            Ok(ListingLookupRound::Paused) => {
                if wait_while_paused(&mut drain).await {
                    return;
                }
            }
            Err(ConsignmentError::LeaseLost) => {
                tracing::debug!(code = "lease_lost", "寄售解析围栏已失效");
                tokio::task::yield_now().await;
            }
            _ => tokio::task::yield_now().await,
        }
    }
}
