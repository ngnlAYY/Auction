//! 退件 Take executor（W-6）：领取接管的唯一 Take 工作 → 起写前只读准备（会话绑定与写前清单）→
//! `start_take`（拿到一次性许可才 POST）→ 读精确 Take 证明 → `consume`。
//!
//! 保护不变量 4、5、6：
//! - POST 许可只由 `ReturnService::start_take` 在存储报告“首次提交”后铸造，以值交给传输执行一次；
//!   重放、新 fence、提交结果未知都拿不到许可，零 POST。
//! - 起写之后结果未知、证明未获证（验证器未签收或证据不足，消费事务保留核对）都不再 Take：
//!   同一工作已有成功起写，之后只能得到“已提交”。
//! - 不按 RTS 主题比对：Take 工作的持久主题是固定占位，本循环只认工作、投递、尝试、fence 与写前
//!   冻结清单摘要；证明是否就是这封原件由消费事务在锁内按持久事实判定。
//!
//! 日志只记错误码，不含身份、装备或凭据。

use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, ConsumeReturnInput, DepositBusinessRejection, PostPermit,
    ReturnService, StartTakeInput, WriteStart,
};
use auction_observability::WorkerResult;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;
use uuid::Uuid;

const IDLE_POLL: Duration = Duration::from_secs(5);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);

/// 一项已领取的 Take 工作：接管、工作、投递与本次尝试的领取身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimedTake {
    /// 接管 ID。
    pub intake_id: Uuid,
    /// 唯一 Take 工作。
    pub work_id: Uuid,
    /// 该工作的 Take 投递。
    pub delivery_id: Uuid,
    /// 本次 attempt。
    pub attempt_id: Uuid,
    /// 当前 fence。
    pub fence: u64,
    /// 当前 claim token。
    pub claim_token: Uuid,
}

/// Take 工作的持久来源；实现方只委托存储，不在内存里保存领取状态。
pub trait ReturnTakeWorkSource: Send + Sync {
    /// 领取下一项待执行的 Take 工作；没有时返回 `None`。
    fn claim_next<'a>(
        &'a self,
        worker_id: &'a str,
        lease: Duration,
    ) -> ConsignmentFuture<'a, Option<ClaimedTake>>;
}

/// 起写前只读准备的结果。
pub struct PreparedTake {
    /// 证据会话绑定摘要。
    pub session_binding_digest: [u8; 32],
    /// 写前读到的冻结清单摘要。
    pub expected_manifest_digest: [u8; 32],
    /// 传输自己的执行句柄；执行时由同一传输取回。
    pub handle: Box<dyn Any + Send>,
}

/// 精确 Take 证明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeProof {
    /// 证明观察。
    pub observation_id: Uuid,
    /// 验证器版本。
    pub validator_version: String,
    /// 证明摘要。
    pub proof_digest: [u8; 32],
}

/// 持许可执行一次 Take 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeExecution {
    /// 读到精确 Take 证明。
    Proven(TakeProof),
    /// Take 已发出但结果无法证明：转人工核对，不重做。
    Unknown,
}

/// 退件 Take 的外写传输；与处置执行共用同一 MoogleMail 会话与凭据独占锁。
pub trait ReturnTakeTransport: Send + Sync {
    /// 起写前的只读准备；`None` 表示当前不可执行，本轮不起写。
    fn prepare<'a>(&'a self, take: &'a ClaimedTake) -> ConsignmentFuture<'a, Option<PreparedTake>>;

    /// 以一次性许可执行唯一一次 Take 并读证明；许可以值移入，结果不明一律 `Unknown`。
    fn execute(
        &self,
        prepared: PreparedTake,
        permit: PostPermit,
    ) -> ConsignmentFuture<'_, TakeExecution>;
}

/// 单轮结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeRound {
    /// 没有待执行的 Take。
    Idle,
    /// 传输暂不可用：稍后再试，未起写。
    Deferred,
    /// 已有成功起写：本 executor 不再 Take。
    Closed,
    /// Take 已证明并消费。
    Consumed,
    /// 证明未获证，消费事务保留核对。
    Unproven,
    /// Take 结果未知，转人工核对。
    Unknown,
}

/// Take executor 的运行参数。
pub struct ReturnTakeRuntime {
    /// 持久工作来源。
    pub work: Arc<dyn ReturnTakeWorkSource>,
    /// 退件接管用例（许可的唯一铸造点）。
    pub service: Arc<ReturnService>,
    /// 外写传输。
    pub transport: Arc<dyn ReturnTakeTransport>,
    /// 当前 Worker 实例名。
    pub worker_id: String,
    /// 每次领取的持久租约时长。
    pub lease_duration: Duration,
    /// 可选唤醒；丢失时由有界轮询恢复。
    pub wakeup: Arc<Notify>,
    /// 外部访问暂停快照；暂停时不领取、不外写。
    pub paused: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl ReturnTakeRuntime {
    /// 处理一项 Take 工作。只有本次起写首次提交时才 Take，且只 Take 一次。
    pub async fn run_once(&self) -> Result<TakeRound, ConsignmentError> {
        let Some(take) = self.work.claim_next(&self.worker_id, self.lease_duration).await? else {
            return Ok(TakeRound::Idle);
        };
        let Some(prepared) = self.transport.prepare(&take).await? else {
            return Ok(TakeRound::Deferred);
        };
        let started = self
            .service
            .start_take(StartTakeInput {
                intake_id: take.intake_id,
                work_id: take.work_id,
                delivery_id: take.delivery_id,
                attempt_id: take.attempt_id,
                fence: take.fence,
                claim_token: take.claim_token,
                session_binding_digest: prepared.session_binding_digest,
                expected_manifest_digest: prepared.expected_manifest_digest,
            })
            .await?;
        let permit = match started {
            WriteStart::FirstCommitted { permit, .. } => permit,
            WriteStart::AlreadyCommitted(_) => return Ok(TakeRound::Closed),
        };
        let proof = match self.transport.execute(prepared, permit).await? {
            TakeExecution::Proven(proof) => proof,
            TakeExecution::Unknown => {
                tracing::warn!(code = "take_write_unknown", "退件 Take 结果未知，转人工核对");
                return Ok(TakeRound::Unknown);
            }
        };
        let consumed = self
            .service
            .consume(ConsumeReturnInput {
                intake_id: take.intake_id,
                original_attempt_id: take.attempt_id,
                fence: take.fence,
                observation_id: proof.observation_id,
                validator_version: proof.validator_version,
                proof_digest: proof.proof_digest,
            })
            .await;
        match consumed {
            Ok(_) => Ok(TakeRound::Consumed),
            Err(ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven)) => {
                tracing::warn!(code = "evidence_unproven", "退件 Take 证明未获证，保留核对");
                Ok(TakeRound::Unproven)
            }
            Err(error) => Err(error),
        }
    }
}

/// 运行 Take executor，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<ReturnTakeRuntime>,
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
        let (worker_result, wait) = match result {
            Ok(TakeRound::Consumed | TakeRound::Unproven | TakeRound::Unknown) => {
                (WorkerResult::Success, Wait::Immediate)
            }
            // 已起写或暂不可执行的项仍会被来源返回：按空闲等待，避免以查询速率空转。
            Ok(TakeRound::Idle | TakeRound::Deferred | TakeRound::Closed) => {
                (WorkerResult::Success, Wait::Idle)
            }
            Err(error) => {
                tracing::warn!(code = error_code(error), "退件 Take 一轮未确认完成");
                (WorkerResult::Error, Wait::Backoff)
            }
        };
        observe(worker_result, started_at.elapsed());
        if stopped(&drain) {
            return;
        }
        match wait {
            Wait::Immediate => {
                backoff = BACKOFF_INITIAL;
                tokio::task::yield_now().await;
            }
            Wait::Idle => {
                backoff = BACKOFF_INITIAL;
                if wait_for_wake(&mut drain, runtime.wakeup.as_ref(), IDLE_POLL).await {
                    return;
                }
            }
            Wait::Backoff => {
                if wait_for_backoff(&mut drain, backoff).await {
                    return;
                }
                backoff = backoff.saturating_mul(2).min(BACKOFF_MAXIMUM);
            }
        }
    }
}

enum Wait {
    Immediate,
    Idle,
    Backoff,
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
