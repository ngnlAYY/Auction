//! 订单复核消费端（方案 A）：领取复核工作 → 对快照里每一条“已起写、无 Sent 绑定”的派发尝试按原 attempt
//! 只读查 Sent → 查到则经既有 `record_evidence` 补记 Sent 绑定 → 按租约完成（结果由存储推导）。
//!
//! 保护不变量 4、5、6：
//! - 复核绝不 POST、不 Take、不起写：读取端口只读，补记只经既有证据写口（同键重放、异摘要冲突由写口判定）；
//! - 工作、租约 / fence、快照与结果都持久在数据库；租约已失（`LeaseLost`）时放弃本轮，结果交给新领取者；
//! - 查不到、读不到或补记失败都不改变事实，完成时由存储推导为 `still_unknown`，留待人工处置。
//! - 已起写未证的 Take 本期没有只读重证读口，不读、不 Take，原样留给完成推导（见 PIWW3 报告）。
//!
//! 日志只记错误码，不含收件人、主题或装备。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DispatchService, OrderRecheckOutcome,
    OrderRecheckWorkStore, RecheckDispatchAttempt, RecordEvidenceInput,
};
use auction_application::core::Clock;
use auction_domain::consignment::dispatch::{AttemptKey, EvidenceCategory};
use auction_observability::WorkerResult;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

use super::dispatch::SentObservation;

const IDLE_POLL: Duration = Duration::from_secs(30);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);

/// 按原 attempt 只读查 Sent 的端口；实现方不得写入远端。
pub trait DispatchSentReader: Send + Sync {
    /// 查这一条派发尝试冻结的那封邮件；找到返回 Sent 观察，找不到或无法确定返回 `None`。
    fn find_sent<'a>(
        &'a self,
        attempt: &'a RecheckDispatchAttempt,
    ) -> ConsignmentFuture<'a, Option<SentObservation>>;
}

/// 单轮结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecheckRound {
    /// 没有待复核的工作。
    Idle,
    /// 已按租约写下（或重放）结果。
    Completed(OrderRecheckOutcome),
    /// 租约已不是当前租约：放弃本轮。
    LeaseLost,
}

/// 复核消费端的运行参数。
pub struct OrderRecheckRuntime {
    /// 复核工作的持久领取与完成。
    pub store: Arc<dyn OrderRecheckWorkStore>,
    /// 只读 Sent 查找。
    pub reader: Arc<dyn DispatchSentReader>,
    /// 派发写用例（只用其既有证据写口）。
    pub service: Arc<DispatchService>,
    /// 领取时刻来源。
    pub clock: Arc<dyn Clock>,
    /// 当前 Worker 实例名。
    pub worker_id: String,
    /// 每次领取的持久租约时长。
    pub lease_duration: Duration,
    /// 可选唤醒；丢失时由有界轮询恢复。
    pub wakeup: Arc<Notify>,
    /// 外部访问暂停快照；暂停时不领取、不读远端。
    pub paused: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl OrderRecheckRuntime {
    /// 处理一条复核工作。
    pub async fn run_once(&self) -> Result<RecheckRound, ConsignmentError> {
        let now = self.clock.now();
        let lease = auction_time::Duration::from_std(self.lease_duration)
            .ok()
            .ok_or(ConsignmentError::Unavailable)?;
        let lease_until = now.checked_add_signed(lease).ok_or(ConsignmentError::Unavailable)?;
        let Some(claim) =
            self.store.claim_due_order_recheck(self.worker_id.clone(), now, lease_until).await?
        else {
            return Ok(RecheckRound::Idle);
        };
        for attempt in &claim.dispatch_attempts {
            self.recheck_dispatch(attempt).await;
        }
        match self.store.complete_order_recheck(claim.lease).await {
            Ok(outcome) => Ok(RecheckRound::Completed(outcome)),
            Err(ConsignmentError::LeaseLost) => {
                tracing::warn!(code = "lease_lost", "订单复核租约已失，放弃本轮");
                Ok(RecheckRound::LeaseLost)
            }
            Err(error) => Err(error),
        }
    }

    /// 查到即经既有写口补记 Sent 绑定；查不到或失败只记日志，结果由完成推导。
    async fn recheck_dispatch(&self, attempt: &RecheckDispatchAttempt) {
        let sent = match self.reader.find_sent(attempt).await {
            Ok(Some(sent)) => sent,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(code = error_code(error), "订单复核读取 Sent 失败");
                return;
            }
        };
        let recorded = self
            .service
            .record_evidence(RecordEvidenceInput {
                attempt: AttemptKey { delivery_id: attempt.delivery_id, fence: attempt.fence },
                category: EvidenceCategory::SentBinding,
                remote_observation_id: sent.sent_mid,
                observed_at: sent.observed_at,
                validator_version: sent.validator_version,
                evidence_digest: sent.evidence_digest,
            })
            .await;
        if let Err(error) = recorded {
            tracing::warn!(code = error_code(error), "订单复核补记 Sent 绑定未成");
        }
    }
}

/// 运行复核消费端，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<OrderRecheckRuntime>,
    mut drain: watch::Receiver<bool>,
    observe: impl Fn(WorkerResult, Duration) + Send + Sync,
) {
    let mut backoff = BACKOFF_INITIAL;
    while !stopped(&drain) {
        if (runtime.paused)() {
            if wait_for_wake(&mut drain, runtime.wakeup.as_ref(), IDLE_POLL).await {
                return;
            }
            continue;
        }
        let started_at = Instant::now();
        let result = runtime.run_once().await;
        observe(
            if result.is_ok() { WorkerResult::Success } else { WorkerResult::Error },
            started_at.elapsed(),
        );
        if stopped(&drain) {
            return;
        }
        match result {
            Ok(RecheckRound::Completed(_)) => {
                backoff = BACKOFF_INITIAL;
                tokio::task::yield_now().await;
            }
            Ok(RecheckRound::Idle | RecheckRound::LeaseLost) => {
                backoff = BACKOFF_INITIAL;
                if wait_for_wake(&mut drain, runtime.wakeup.as_ref(), IDLE_POLL).await {
                    return;
                }
            }
            Err(error) => {
                tracing::warn!(code = error_code(error), "订单复核一轮未确认完成");
                if wait_for_backoff(&mut drain, backoff).await {
                    return;
                }
                backoff = backoff.saturating_mul(2).min(BACKOFF_MAXIMUM);
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
