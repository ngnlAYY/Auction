//! 派发、补寄、下游首轮与退件 Take 的持久工作来源适配：把 jobs 窄接口一一委托给存储端口。
//!
//! 不变量 5：领取、租约、fence 与补寄待办全部来自数据库，本文件不在内存保存任何工作状态；
//! 类型转换之外不做业务判断。补寄的新轮投递尚未备好时，按处置记下的轮 ID 交存储备好（D0 由存储
//! 在备好事务里按裁定计算），备好失败关闭的项本轮跳过。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DispatchWorkStore, QueuedRedispatch,
};
use auction_application::core::Clock;
use auction_application::mail::{ReturnTakeClaim, ReturnTakeWorkStore};
use auction_domain::consignment::dispatch::{AttemptKey, dispatch_round_id};
use auction_jobs::consignment::{
    ClaimedTake, DispatchLease, DispatchWork, DispatchWorkSource, DownstreamRoundSource,
    PendingDownstreamRound, PendingRedispatch, RedispatchWorkSource, ReturnTakeWorkSource,
};
use auction_storage::{DatabaseStore, DownstreamRoundRepository, RoundLease, RoundLeaseRequest};
use auction_time::Timestamp;

/// 派发工作来源：领取寄售义务，并把履约行衔接到轮投递。
pub struct StoreDispatchWork {
    /// 衔接与投递租约（存储写端）。
    pub store: Arc<DatabaseStore>,
    /// 寄售义务领取（派发工作端口）。
    pub work: Arc<dyn DispatchWorkStore>,
    /// 领取时刻来源。
    pub clock: Arc<dyn Clock>,
}

impl DispatchWorkSource for StoreDispatchWork {
    fn claim_next<'a>(
        &'a self,
        _worker_id: &'a str,
        lease: Duration,
    ) -> ConsignmentFuture<'a, Option<DispatchWork>> {
        Box::pin(async move {
            let (now, lease_until) = lease_window(self.clock.as_ref(), lease)?;
            let claim = self.work.claim_dispatch_work(now, lease_until).await?;
            Ok(claim.map(|claim| DispatchWork {
                obligation_id: claim.obligation_id,
                claim_token: claim.claim_token,
                claim_fencing: claim.claim_fencing,
                key: claim.key,
                frozen: claim.frozen,
            }))
        })
    }

    fn lease_round<'a>(&'a self, work: &'a DispatchWork) -> ConsignmentFuture<'a, DispatchLease> {
        Box::pin(async move {
            let request = RoundLeaseRequest {
                obligation_id: work.obligation_id,
                claim_token: work.claim_token,
                claim_fencing: work.claim_fencing,
                round_id: dispatch_round_id(&work.key),
            };
            Ok(match self.store.lease_dispatch_round(&request).await? {
                RoundLease::Leased { delivery_id, claim_token, fence } => DispatchLease::Leased {
                    attempt: AttemptKey { delivery_id, fence },
                    claim_token,
                },
                RoundLease::Busy => DispatchLease::Busy,
                RoundLease::Settled => DispatchLease::Settled,
                RoundLease::Rejected => DispatchLease::Rejected,
            })
        })
    }
}

/// 补寄待办来源：已排队、尚未建轮的补寄处置，连同新轮已备好的投递与冻结身份。
pub struct StoreRedispatchWork {
    /// 补寄待办读口与新轮投递备好写端。
    pub work: Arc<dyn DispatchWorkStore>,
}

impl RedispatchWorkSource for StoreRedispatchWork {
    fn pending_redispatches(&self, limit: u32) -> ConsignmentFuture<'_, Vec<PendingRedispatch>> {
        Box::pin(async move {
            let queued = self.work.queued_redispatches(limit).await?;
            let mut pending = Vec::with_capacity(queued.len());
            for item in queued {
                if let Some(ready) = self.prepared(item).await? {
                    pending.push(ready);
                }
            }
            Ok(pending)
        })
    }
}

impl StoreRedispatchWork {
    /// 已备好的直接给出；未备好的按处置的轮 ID 交存储备好，失败关闭（`None`）的本轮跳过。
    async fn prepared(
        &self,
        item: QueuedRedispatch,
    ) -> Result<Option<PendingRedispatch>, ConsignmentError> {
        let (delivery_id, frozen) = match item.prepared {
            Some(prepared) => (prepared.delivery_id, prepared.frozen),
            None => match self.work.prepare_redispatch(item.dispatch_id).await? {
                Some(prepared) => (prepared.delivery_id, prepared.frozen),
                None => {
                    tracing::warn!(
                        code = "redispatch_prepare_refused",
                        "补寄新轮投递备好被拒（内容或来源不符），等待人工核对"
                    );
                    return Ok(None);
                }
            },
        };
        Ok(Some(PendingRedispatch {
            dispatch_id: item.dispatch_id,
            key: item.key,
            closed_by_return_intake: item.closed_by_return_intake,
            delivery_id,
            frozen,
        }))
    }
}

/// 下游首轮来源：存储列出已放行、尚未建轮的下游义务并备好首轮投递（备好失败关闭的项由存储跳过）。
pub struct StoreDownstreamRounds {
    /// 下游首轮生产者（存储写端）。
    pub repository: DownstreamRoundRepository,
}

impl DownstreamRoundSource for StoreDownstreamRounds {
    fn pending_downstream_rounds(
        &self,
        limit: u32,
    ) -> ConsignmentFuture<'_, Vec<PendingDownstreamRound>> {
        Box::pin(async move {
            let prepared = self.repository.prepare_pending(limit).await?;
            Ok(prepared
                .into_iter()
                .map(|round| PendingDownstreamRound {
                    key: round.key,
                    delivery_id: round.delivery_id,
                    frozen: round.frozen,
                })
                .collect())
        })
    }
}

/// 退件 Take 工作来源：只领退件接管建的唯一 Take 工作。
pub struct StoreReturnTakeWork {
    /// 退件 Take 领取端口。
    pub store: Arc<dyn ReturnTakeWorkStore>,
    /// 领取时刻来源。
    pub clock: Arc<dyn Clock>,
}

impl ReturnTakeWorkSource for StoreReturnTakeWork {
    fn claim_next<'a>(
        &'a self,
        worker_id: &'a str,
        lease: Duration,
    ) -> ConsignmentFuture<'a, Option<ClaimedTake>> {
        Box::pin(async move {
            let (now, lease_until) = lease_window(self.clock.as_ref(), lease)?;
            let claim =
                self.store.claim_due_return_take(worker_id.to_owned(), now, lease_until).await?;
            Ok(claim.and_then(claimed_take))
        })
    }
}

/// 存储返回的领取形状不合法时不交给 executor（失败关闭）；租约到期后由存储收回。
fn claimed_take(claim: ReturnTakeClaim) -> Option<ClaimedTake> {
    if !claim.validate() {
        tracing::warn!(code = "return_take_claim_invalid", "退件 Take 领取形状不合法，不执行");
        return None;
    }
    Some(ClaimedTake {
        intake_id: claim.intake_id,
        work_id: claim.work.work_id,
        delivery_id: claim.work.delivery_id,
        attempt_id: claim.work.attempt_id,
        fence: claim.work.fence,
        claim_token: claim.work.claim_token,
    })
}

/// 领取时刻与租约截止；时长或截止溢出时失败关闭，不领取。
fn lease_window(
    clock: &dyn Clock,
    lease: Duration,
) -> Result<(Timestamp, Timestamp), ConsignmentError> {
    let now = clock.now();
    let lease =
        auction_time::Duration::from_std(lease).ok().ok_or(ConsignmentError::Unavailable)?;
    let lease_until = now.checked_add_signed(lease).ok_or(ConsignmentError::Unavailable)?;
    Ok((now, lease_until))
}
