//! 寄售派发 executor（W-5）：领取寄售义务 → 衔接并领取轮投递 → 起写前只读准备 → 登记尝试 →
//! 持久起写（拿到一次性许可才 POST）→ 经投递传输发送并读 Sent → 追加 Sent 绑定证据。
//!
//! 保护不变量 4、5、6：
//! - POST 许可只由 `DispatchService::mark_write_started` 在存储报告“首次提交”后铸造，以值交给传输
//!   执行一次；重放、新 fence、提交结果未知都拿不到许可，零 POST。本循环不缓存、不重建许可。
//! - 起写之后的结果未知（远端无法证明、Sent 读不到、证据没能落库）一律留在“已起写、无终局证据”的
//!   持久状态转人工核对，绝不再发：投递已进入 `sending`，重启或并发领取者衔接时只得到“已终结”。
//! - 待办、租约与 fence 全部来自数据库；内存只做唤醒与退避。
//!
//! 日志只记种类与错误码，不含收件人、装备、预约码或凭据。

use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DispatchService, PostPermit, RecordEvidenceInput,
    WriteStart,
};
use auction_application::delivery::DeliverySendCredentialBinding;
use auction_application::settlement::SettlementInternalEvidenceSnapshot;
use auction_domain::consignment::dispatch::{
    AttemptKey, DispatchRoundKey, EvidenceCategory, FrozenDispatchIdentity,
};
use auction_observability::WorkerResult;
use auction_time::Timestamp;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;
use uuid::Uuid;

const IDLE_POLL: Duration = Duration::from_secs(5);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);

/// 一项已领取的寄售派发工作：寄售义务的履约 claim 与要派发的轮。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchWork {
    /// 被派发的寄售义务。
    pub obligation_id: Uuid,
    /// 义务领取写在履约行上的 token。
    pub claim_token: Uuid,
    /// 义务领取写在履约行上的 fence。
    pub claim_fencing: i64,
    /// 要派发的轮。
    pub key: DispatchRoundKey,
    /// 轮上冻结的派发身份。
    pub frozen: FrozenDispatchIdentity,
}

/// 履约行衔接到轮投递并领取该投递的结果（与存储 `RoundLease` 一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchLease {
    /// 取得投递租约：以尝试键与租约 token 登记尝试并提交起写。
    Leased {
        /// 投递与 fence。
        attempt: AttemptKey,
        /// 投递租约 token。
        claim_token: Uuid,
    },
    /// 投递已有未过期的租约（并发领取者）：本轮不动，过租后再试。
    Busy,
    /// 投递已起写或已终结：不再发，结果由证据与人工核对收口。
    Settled,
    /// 履约 claim 失效、轮或投递不符、受益人保留权利：失败关闭。
    Rejected,
}

/// 寄售派发工作的持久来源；实现方只委托存储，不在内存里保存领取状态。
pub trait DispatchWorkSource: Send + Sync {
    /// 领取下一项待派发的寄售义务；没有时返回 `None`。
    fn claim_next<'a>(
        &'a self,
        worker_id: &'a str,
        lease: Duration,
    ) -> ConsignmentFuture<'a, Option<DispatchWork>>;

    /// 把义务的履约行衔接到轮投递并领取该投递。
    fn lease_round<'a>(&'a self, work: &'a DispatchWork) -> ConsignmentFuture<'a, DispatchLease>;
}

/// 起写前只读准备的结果：发送所用的 active 凭据绑定、现金腿的同会话资金证据与只能交回同一传输的不透明句柄。
pub struct PreparedDispatch {
    /// 起写事务锁验并冻结的凭据绑定。
    pub credential: DeliverySendCredentialBinding,
    /// 现金腿（冻结 Credits 大于零）在发信会话里取得的新鲜资金证据，起写事务按通用现金门核对；
    /// 非现金腿为 `None`。现金腿缺证据时存储失败关闭，不发许可。
    pub cash_evidence: Option<SettlementInternalEvidenceSnapshot>,
    /// 传输自己的发送句柄；执行时由同一传输取回。
    pub handle: Box<dyn Any + Send>,
}

/// Sent 箱里读到的本次写入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentObservation {
    /// 唯一 Sent MID。
    pub sent_mid: String,
    /// 观察时刻。
    pub observed_at: Timestamp,
    /// 产出该观察的验证器版本。
    pub validator_version: String,
    /// 证据摘要。
    pub evidence_digest: [u8; 32],
}

/// 持许可执行一次 POST 并读 Sent 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchExecution {
    /// Sent 箱证明了本次写入。
    Proven(SentObservation),
    /// 写入已开始但结果无法证明（超时、断连、Sent 读不到）：转人工核对，不重发。
    Unknown,
}

/// 寄售派发的外写传输；与通用投递共用同一 MoogleMail 发信入口与凭据独占锁。
pub trait DispatchTransport: Send + Sync {
    /// 起写前的只读准备（凭据、compose 空闲、现金腿的同会话资金证据等），不写远端；`None` 表示当前
    /// 不可发，本轮不起写、不登记尝试。`claim_token` 是投递租约 token，资金证据绑定它与尝试键。
    fn prepare<'a>(
        &'a self,
        work: &'a DispatchWork,
        attempt: AttemptKey,
        claim_token: Uuid,
    ) -> ConsignmentFuture<'a, Option<PreparedDispatch>>;

    /// 以一次性许可执行唯一一次 POST 并读 Sent；许可以值移入，结果不明一律 `Unknown`。
    fn execute(
        &self,
        prepared: PreparedDispatch,
        permit: PostPermit,
    ) -> ConsignmentFuture<'_, DispatchExecution>;
}

/// 单轮结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchRound {
    /// 没有待派发的义务。
    Idle,
    /// 投递正被别人持有或传输暂不可发：稍后再试。
    Deferred,
    /// 已起写、已终结或被拒：本 executor 不再对它外写。
    Closed,
    /// 本轮唯一一次 POST 已由 Sent 证明并落库。
    Sent,
    /// 本轮 POST 结果未知：留在已起写状态转人工核对。
    Unknown,
}

/// 派发 executor 的运行参数。
pub struct ConsignmentDispatchRuntime {
    /// 持久工作来源。
    pub work: Arc<dyn DispatchWorkSource>,
    /// 派发写用例（许可的唯一铸造点）。
    pub service: Arc<DispatchService>,
    /// 外写传输。
    pub transport: Arc<dyn DispatchTransport>,
    /// 当前 Worker 实例名。
    pub worker_id: String,
    /// 每次领取义务的持久租约时长。
    pub lease_duration: Duration,
    /// 可选唤醒；丢失时由有界轮询恢复。
    pub wakeup: Arc<Notify>,
    /// 外部访问暂停快照；暂停时不领取、不外写。
    pub paused: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl ConsignmentDispatchRuntime {
    /// 处理一项工作。只有本次起写首次提交时才 POST，且只 POST 一次。
    pub async fn run_once(&self) -> Result<DispatchRound, ConsignmentError> {
        let Some(work) = self.work.claim_next(&self.worker_id, self.lease_duration).await? else {
            return Ok(DispatchRound::Idle);
        };
        let (attempt, claim_token) = match self.work.lease_round(&work).await? {
            DispatchLease::Leased { attempt, claim_token } => (attempt, claim_token),
            DispatchLease::Busy => return Ok(DispatchRound::Deferred),
            DispatchLease::Settled => return Ok(DispatchRound::Closed),
            DispatchLease::Rejected => {
                tracing::warn!(code = "dispatch_lease_rejected", "寄售派发衔接被拒，失败关闭");
                return Ok(DispatchRound::Closed);
            }
        };
        let Some(prepared) = self.transport.prepare(&work, attempt, claim_token).await? else {
            // 尚未起写：租约到期后可由任一 executor 重新领取，不产生任何外写。
            return Ok(DispatchRound::Deferred);
        };
        self.service.start_attempt(work.key, attempt, claim_token, work.frozen.clone()).await?;
        let credential = Some(prepared.credential.clone());
        // 现金腿把同会话资金证据借给起写事务核对；证据只决定首次提交，不进摘要（不变量 4、6）。
        let started = match &prepared.cash_evidence {
            Some(evidence) => {
                self.service
                    .mark_cash_write_started(attempt, claim_token, credential, evidence)
                    .await?
            }
            None => self.service.mark_write_started(attempt, claim_token, credential).await?,
        };
        let permit = match started {
            WriteStart::FirstCommitted { permit, .. } => permit,
            WriteStart::AlreadyCommitted(_) => return Ok(DispatchRound::Closed),
        };
        match self.transport.execute(prepared, permit).await? {
            DispatchExecution::Proven(sent) => {
                self.service
                    .record_evidence(RecordEvidenceInput {
                        attempt,
                        category: EvidenceCategory::SentBinding,
                        remote_observation_id: sent.sent_mid,
                        observed_at: sent.observed_at,
                        validator_version: sent.validator_version,
                        evidence_digest: sent.evidence_digest,
                    })
                    .await
                    .inspect_err(|error| {
                        // 已证发出却没能落库：投递已在 sending，不会再发；观察只在本进程内存里，
                        // 由人工核对按 Sent 箱补证。
                        tracing::warn!(code = error_code(*error), "寄售派发已证发出但证据未落库");
                    })?;
                Ok(DispatchRound::Sent)
            }
            DispatchExecution::Unknown => {
                tracing::warn!(code = "dispatch_write_unknown", "寄售派发结果未知，转人工核对");
                Ok(DispatchRound::Unknown)
            }
        }
    }
}

/// 运行派发 executor，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<ConsignmentDispatchRuntime>,
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
            Ok(DispatchRound::Sent | DispatchRound::Unknown) => {
                (WorkerResult::Success, Wait::Immediate)
            }
            // 被持有、已终结或被拒的项仍会被来源返回：按空闲等待，避免以查询速率空转。
            Ok(DispatchRound::Idle | DispatchRound::Deferred | DispatchRound::Closed) => {
                (WorkerResult::Success, Wait::Idle)
            }
            Err(error) => {
                tracing::warn!(code = error_code(error), "寄售派发一轮未确认完成");
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
