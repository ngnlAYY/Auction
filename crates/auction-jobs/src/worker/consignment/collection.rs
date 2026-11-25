//! 收款与确领推进循环（PIWQ，D9 §2.3、§4.2、§6.2）：对已成功派发、已绑定 Sent 的寄售轮，按冻结身份只读观察
//! Sent 详情的领取状态；读到可证的领取事实时经既有写口记录证据并推进——main 原件 COD：付款认证（`certify_cod`）
//! 后确领原件（`prove_handoff`，同事务放行卖款）；isekai 载体 COD：付款认证（同事务放行原件交割）；isekai 原件
//! 交割：确领原件；卖款与退款：现金确领证据（`record_evidence` 同事务记 `claimed`）。
//!
//! 保护不变量 1、4、5、6：
//! - 只读远端：观察端口只读 Sent 详情，不 POST、不 Take；本循环没有任何外写能力。
//! - 验证器未签收即不读、不推进：观察前先问存储该验证器版本是否已签收（与写口同一份清单），未签收时零读取、
//!   零证据、零推进。失败关闭的第二道在各写口里：现金确领由 `record_evidence` 复核签收（未签收只记观察、不记
//!   `claimed`）；原件与载体付款由 `certify_cod` 读付款证据时复核（`receipt.rs` 的 `remote_payment`），原件确领
//!   由 `prove_handoff` 复核（`original_claim_signed`）。
//! - 全部幂等：证据按尝试、类别与远端观察身份去重；认证与确领的命令 ID 只由原义务与原投递 / 原轮决定，重放、
//!   重启与并发 Worker 只读原结果；已记录的已签收证据直接复用，不重读远端。待办与进度全部持久在数据库；一次扫描
//!   内按 `(started_at, attempt_id)` 键集分页走完全部待办，游标只活在本次扫描里，不跨轮、不作为任何事实来源。
//! - 读不到、未领取或证据不足（写口按临时缺证回滚）都照旧等待，不改事实；严格现金认证缺新鲜现金观察时不认证。
//! - 外部访问暂停时不读远端（含现金观察）；只用已落库证据的推进照常。寄售关闭时有存量照常运行（续办已成交订单）。
//!
//! 日志只记错误码，不含收件人、主题、装备或 Sent MID。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    CollectedEvidence, CollectionCursor, CollectionLeg, CollectionPage, ConsignmentError,
    ConsignmentFuture, DispatchService, PendingCollection, ProveHandoffInput, RecordEvidenceInput,
    certify_cod_command,
};
use auction_application::settlement::{
    ConsignmentFundingStore, FreshCashObservation, FundingError,
};
use auction_domain::consignment::dispatch::EvidenceCategory;
use auction_domain::consignment::funding::FundingMode;
use auction_observability::WorkerResult;
use auction_time::Timestamp;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

use super::stopped;

/// 无进展时的轮询间隔：等待买家或受益人领取可能持续数天，有界轮询即可，不需要更密的远端读取。
const IDLE_POLL: Duration = Duration::from_secs(300);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(60);
/// 生产装配的每页待办数；一次扫描按页走完全部待办。
pub const COLLECTION_PAGE_SIZE: u32 = 32;

/// 收款与确领待办的持久来源；实现方只委托存储。
pub trait CollectionSource: Send + Sync {
    /// 游标之后按起写先后列出一页已成功派发、已绑定 Sent、还差下一步的轮；读满一页才给出下一页游标。
    fn pending_collections(
        &self,
        after: Option<CollectionCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, CollectionPage>;

    /// 该类别下该验证器版本是否已签收（与写口同一份清单）。
    fn validator_signed(&self, category: EvidenceCategory, version: &str) -> bool;
}

/// 一次已领取的只读观察。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionObservation {
    /// 远端观察 ID（被领取的那封 Sent 的 MID）。
    pub remote_observation_id: String,
    /// 观察时刻。
    pub observed_at: Timestamp,
    /// 证据摘要；同一领取事实重读不变。
    pub evidence_digest: [u8; 32],
}

/// 只读观察的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionRead {
    /// 身份逐字相符、资产已全部领取。
    Collected(CollectionObservation),
    /// 仍有剩余资产：照旧等待。
    Pending,
    /// 读取失败或身份不符：无法确定，照旧等待。
    Unavailable,
}

/// 只读观察端口；实现方不得写入远端。
pub trait CollectionObserver: Send + Sync {
    /// 产出观察的验证器版本（固定）；循环据此先判签收，未签收时不调用 [`CollectionObserver::observe`]。
    fn validator_version(&self) -> &str;

    /// 按已绑定的 Sent MID 与冻结身份读取领取状态。
    fn observe<'a>(
        &'a self,
        pending: &'a PendingCollection,
    ) -> ConsignmentFuture<'a, CollectionRead>;

    /// 严格现金认证所需的新鲜 main 现金观察（认证 UID 必须等于 `uid`）；取不到返回 `None`，不认证。
    fn fresh_main_cash<'a>(
        &'a self,
        uid: &'a str,
    ) -> ConsignmentFuture<'a, Option<FreshCashObservation>>;
}

/// 推进循环的运行参数。
pub struct CollectionRuntime {
    /// 持久待办来源。
    pub source: Arc<dyn CollectionSource>,
    /// 只读 Sent 观察。
    pub observer: Arc<dyn CollectionObserver>,
    /// 派发写用例（证据与确领写口）。
    pub service: Arc<DispatchService>,
    /// 资金写端口（收款认证写口）。
    pub funding: Arc<dyn ConsignmentFundingStore>,
    /// 可选唤醒；丢失时由有界轮询恢复。
    pub wakeup: Arc<Notify>,
    /// 外部访问暂停快照；暂停时不读远端。
    pub paused: Arc<dyn Fn() -> bool + Send + Sync>,
    /// 每页待办数（生产为 [`COLLECTION_PAGE_SIZE`]）。
    pub page_size: u32,
}

/// 单轮结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CollectionRound {
    /// 本轮首次提交（非重放）的写入数：证据、认证与确领各计一次。
    pub advanced: usize,
    /// 读取待办失败或写口返回非业务错误。
    pub failed: bool,
}

impl CollectionRuntime {
    /// 一次扫描：按 `(started_at, attempt_id)` 键集分页走完全部待办，读不满一页即结束。游标只在本次扫描内
    /// 使用，不跨轮、不作为事实来源；推进不了的轮不会让后面的轮饿死。
    pub async fn run_once(&self) -> CollectionRound {
        let mut round = CollectionRound::default();
        let mut after = None;
        loop {
            let page = match self.source.pending_collections(after, self.page_size).await {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(code = error_code(error), "读取收款确领待办失败，稍后重试");
                    round.failed = true;
                    return round;
                }
            };
            for item in page.items {
                match self.advance(item).await {
                    Ok(advanced) => round.advanced += advanced,
                    Err(Failure) => round.failed = true,
                }
            }
            // 游标必须严格前进；来源给出不前进的游标说明实现有误，结束本次扫描而不是空转。
            match page.next {
                Some(next) if after != Some(next) => after = Some(next),
                _ => return round,
            }
        }
    }

    /// 推进一轮；返回首次提交的写入数。业务拒绝与临时缺证只记安全码并等待下一轮。
    async fn advance(&self, item: PendingCollection) -> Result<usize, Failure> {
        let mut advanced = 0;
        let evidence = match item.evidence.clone() {
            Some(evidence) => evidence,
            None => match self.observe(&item).await? {
                Some((evidence, recorded)) => {
                    advanced += recorded;
                    evidence
                }
                None => return Ok(advanced),
            },
        };
        match item.leg {
            // 现金确领证据在 `record_evidence` 同事务记 `claimed`，没有下一步。
            CollectionLeg::Cash => {}
            CollectionLeg::CarrierCod => {
                advanced += self.certify(&item, &evidence).await?.unwrap_or(0);
            }
            CollectionLeg::MainOriginalCod => {
                // main 原件 COD：付款先认证，确领写口要求付款腿已满足；认证未成时本轮不确领。
                if item.certify.is_some() {
                    match self.certify(&item, &evidence).await? {
                        Some(recorded) => advanced += recorded,
                        None => return Ok(advanced),
                    }
                }
                advanced += self.prove_handoff(&item, &evidence).await?;
            }
            CollectionLeg::OriginalHandoff => {
                advanced += self.prove_handoff(&item, &evidence).await?;
            }
        }
        Ok(advanced)
    }

    /// 只读观察并记录证据；验证器未签收或外部访问暂停时不读。返回证据与是否首次记录。
    async fn observe(
        &self,
        item: &PendingCollection,
    ) -> Result<Option<(CollectedEvidence, usize)>, Failure> {
        let category = item.leg.category();
        let version = self.observer.validator_version().to_owned();
        // 签收前零读取、零证据（D9 §2.3、R5-C10）：未签收的观察即使记录也不能成为结论，反而会占住证据键。
        if (self.paused)() || !self.source.validator_signed(category, &version) {
            return Ok(None);
        }
        let observation = match self.observer.observe(item).await {
            Ok(CollectionRead::Collected(observation)) => observation,
            Ok(CollectionRead::Pending | CollectionRead::Unavailable) => return Ok(None),
            Err(error) => {
                tracing::warn!(code = error_code(error), "收款确领观察失败，稍后重试");
                return Ok(None);
            }
        };
        let recorded = self
            .service
            .record_evidence(RecordEvidenceInput {
                attempt: item.attempt,
                category,
                remote_observation_id: observation.remote_observation_id.clone(),
                observed_at: observation.observed_at,
                validator_version: version,
                evidence_digest: observation.evidence_digest,
            })
            .await;
        match recorded {
            Ok(receipt) => Ok(Some((
                CollectedEvidence {
                    remote_observation_id: observation.remote_observation_id,
                    evidence_digest: observation.evidence_digest,
                },
                usize::from(!receipt.replayed),
            ))),
            Err(error) => consignment_failure(error, "收款确领证据未记录"),
        }
    }

    /// 付款认证；返回 `Some(是否首次)` 表示付款已认证（含重放），`None` 表示本轮未认证、等待。
    async fn certify(
        &self,
        item: &PendingCollection,
        evidence: &CollectedEvidence,
    ) -> Result<Option<usize>, Failure> {
        let Some(facts) = item.certify.clone() else {
            return Ok(None);
        };
        let cash = match facts.mode {
            FundingMode::RecordOnly => None,
            FundingMode::StrictCash => {
                if (self.paused)() {
                    return Ok(None);
                }
                match self.observer.fresh_main_cash(&facts.receipt.uid).await {
                    Ok(Some(cash)) => Some(cash),
                    Ok(None) => return Ok(None),
                    Err(error) => {
                        tracing::warn!(code = error_code(error), "收款认证取现金观察失败");
                        return Ok(None);
                    }
                }
            }
        };
        match self.funding.certify_cod(certify_cod_command(facts, evidence, cash)).await {
            Ok(receipt) => Ok(Some(usize::from(!receipt.replayed))),
            Err(error) => {
                tracing::warn!(code = error.safe_kind(), "收款认证未成，等待下一轮");
                // 临时缺证、冲突与不可用都不改事实、等下一轮；存储失败（提交可能未知）记失败退避，下一轮以同一
                // 命令 ID 重放读取原结果。
                if matches!(error, FundingError::Storage | FundingError::SerializationConflict) {
                    Err(Failure)
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// 确领原件（同事务放行卖款或完成交割）；返回是否首次。
    async fn prove_handoff(
        &self,
        item: &PendingCollection,
        evidence: &CollectedEvidence,
    ) -> Result<usize, Failure> {
        let proven = self
            .service
            .prove_handoff(ProveHandoffInput {
                key: item.key,
                attempt: item.attempt,
                remote_observation_id: evidence.remote_observation_id.clone(),
                evidence_digest: evidence.evidence_digest,
            })
            .await;
        match proven {
            Ok(receipt) => Ok(usize::from(!receipt.replayed)),
            Err(error) => consignment_failure::<()>(error, "原件确领未成，等待下一轮").map(|_| 0),
        }
    }
}

/// 写口返回非业务错误（存储不可用等）：本轮记失败以退避。
#[derive(Debug, Clone, Copy)]
struct Failure;

/// 业务拒绝与临时缺证只记安全码、等待下一轮；其余错误记失败。
fn consignment_failure<T>(
    error: ConsignmentError,
    message: &'static str,
) -> Result<Option<T>, Failure> {
    // 确定拒绝、临时缺证与并发者已用同一命令 ID 都只是“本轮不推进”；下一轮从持久事实重新判断。
    let business = matches!(
        error,
        ConsignmentError::Deposit(_)
            | ConsignmentError::Business(_)
            | ConsignmentError::CommandIdConflict
    );
    tracing::warn!(code = error_code(error), "{message}");
    if business { Ok(None) } else { Err(Failure) }
}

/// 运行推进循环，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<CollectionRuntime>,
    mut drain: watch::Receiver<bool>,
    observe: impl Fn(WorkerResult, Duration) + Send + Sync,
) {
    let mut backoff = BACKOFF_INITIAL;
    while !stopped(&drain) {
        let started_at = Instant::now();
        let round = runtime.run_once().await;
        observe(
            if round.failed { WorkerResult::Error } else { WorkerResult::Success },
            started_at.elapsed(),
        );
        let delay = if round.failed {
            let delay = backoff;
            backoff = backoff.saturating_mul(2).min(BACKOFF_MAXIMUM);
            delay
        } else if round.advanced > 0 {
            backoff = BACKOFF_INITIAL;
            tokio::task::yield_now().await;
            continue;
        } else {
            backoff = BACKOFF_INITIAL;
            IDLE_POLL
        };
        if wait(&mut drain, runtime.wakeup.as_ref(), delay, !round.failed).await {
            return;
        }
    }
}

/// 等待唤醒（失败退避时不接受唤醒）、超时或 drain；返回 true 表示应停止。
async fn wait(
    drain: &mut watch::Receiver<bool>,
    wakeup: &Notify,
    delay: Duration,
    wakeable: bool,
) -> bool {
    tokio::select! {
        biased;
        changed = drain.changed() => changed.is_err() || *drain.borrow(),
        () = wakeup.notified(), if wakeable => false,
        () = tokio::time::sleep(delay) => false,
    }
}

fn error_code(error: ConsignmentError) -> &'static str {
    error.api_error().map_or("consignment_unavailable", |(_, code)| code)
}
