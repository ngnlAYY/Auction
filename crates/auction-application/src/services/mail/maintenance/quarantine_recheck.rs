//! 隔离重检：领取持久工作、只读重读原信并追加观察；不产生任何资产工作。

use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use auction_domain::SettlementWorld;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{EnvelopeObservation, LegacyAuctionOwner, ReleaseKind};
use auction_time::Timestamp;
use uuid::Uuid;

use super::disposition::DispositionRoundRequest;
use super::frozen_manifest::{
    DispositionReconcileReason, DispositionRetryReason, disposition_retry_at,
};
use crate::ports::consignment::deposit::ObservationReceipt;
use crate::ports::consignment::{AutoReleaseBinding, ConsignmentError, ConsignmentFuture};
use crate::ports::settlement::MailAccountEvidenceBinding;
use crate::services::consignment::DepositCommandService;

/// 隔离重检需要重读的原信定位；由 4C 从原始观察持久化。
#[derive(Clone, PartialEq, Eq)]
pub struct QuarantineMailLocator {
    /// 世界。
    pub world: SettlementWorld,
    /// 远端 MID。
    pub external_mail_id: String,
    /// 列表发件人投影。
    pub sender: String,
    /// 列表主题投影。
    pub subject: String,
    /// 原始收件分钟文本。
    pub received_minute_raw: String,
    /// 原始收件时刻。
    pub received_at: Timestamp,
}

impl fmt::Debug for QuarantineMailLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuarantineMailLocator")
            .field("world", &self.world)
            .field("external_mail_id", &self.external_mail_id)
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("received_at", &self.received_at)
            .finish()
    }
}

/// 已领取的隔离重检工作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineRecheckClaim {
    /// 工作 UUID。
    pub work_id: Uuid,
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 领取时的隔离版本。
    pub quarantine_version: u64,
    /// 稳定原信键。
    pub mail_key: OriginalMailKey,
    /// 用来重读详情。
    pub locator: QuarantineMailLocator,
    /// 隔离内最新观察；重读结果必须与它同摘要，追加时复用该观察行。
    pub previous_observation_id: Uuid,
    /// 既往认证绑定；用于同名换 UID 判定。
    pub previous_binding: Option<MailAccountEvidenceBinding>,
    /// 当前 claim token。
    pub claim_token: Uuid,
    /// 该工作第几次被领取；用于计算重试到期时间。
    pub attempt_count: u32,
    /// 工作 fence。
    pub fence: u64,
    /// 租约截止。
    pub lease_until: Timestamp,
}

impl QuarantineRecheckClaim {
    /// 校验工作形状。
    pub fn validate(&self) -> bool {
        !self.work_id.is_nil()
            && !self.quarantine_id.is_nil()
            && self.quarantine_version > 0
            && !self.claim_token.is_nil()
            && self.attempt_count > 0
            && self.fence > 0
            && self.lease_until.timestamp_nanos_opt().is_some()
            && !self.locator.external_mail_id.is_empty()
            && self.mail_key.world() == self.locator.world
            && !self.previous_observation_id.is_nil()
    }
}

/// 一次只读重读的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineReobservation {
    /// 远端观察身份；同内容重放必须稳定。
    pub remote_observation_id: String,
    /// 观察时刻。
    pub observed_at: Timestamp,
    /// 4D1 观察经应用层补齐后的领域观察。
    pub observation: EnvelopeObservation,
    /// 受控证据引用。
    pub evidence_ref: Option<String>,
}

/// 隔离重检只读失败闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuarantineRecheckReadError {
    /// 临时限流、战斗或读不到。
    TemporaryUnavailable,
    /// 原读取 claim 已失效。
    LeaseLost,
    /// 观察形状不可用。
    InvalidObservation,
}

/// 隔离重检只是只读重读原信，不创建资产动作。
///
/// 读取器（4D1 适配器，由集成人接入）必须在把原信转成观察之前先运行拍卖归属判据，并把
/// 结果作为 `EnvelopeObservation::legacy_auction_owner` 原样放进观察。命中后的“在原 K
/// 交接”由 4C 的 `release_quarantine`（`auction_return_case` / `auction_recovery_return`）
/// 完成，不在本用例内处理；本用例只追加观察，不改变隔离结论。
pub trait QuarantineRecheckReader: Send + Sync {
    fn reread(
        &self,
        claim: &QuarantineRecheckClaim,
        deadline: Instant,
    ) -> ConsignmentFuture<'_, Result<QuarantineReobservation, QuarantineRecheckReadError>>;
}

/// 隔离重检工作仓储。
pub trait QuarantineRecheckStore: Send + Sync {
    /// 领取一条 due 重检工作。
    fn claim_due(
        &self,
        worker_id: String,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> ConsignmentFuture<'_, Option<QuarantineRecheckClaim>>;

    /// 只读核对 claim/fence。
    fn check_current(&self, claim: &QuarantineRecheckClaim) -> ConsignmentFuture<'_, ()>;

    /// 写前安全释放；仍以原 token/fence 条件更新。
    ///
    /// `retry_at` 是持久到期时间，内存定时器只负责唤醒（不变量 5）；实现必须持久化它，
    /// 并在早于该时刻时不让 `claim_due` 返回本项。
    fn release_for_retry(
        &self,
        claim: &QuarantineRecheckClaim,
        reason: DispositionRetryReason,
        retry_at: Timestamp,
    ) -> ConsignmentFuture<'_, ()>;

    /// 只在观察追加成功后收口工作。
    fn complete(
        &self,
        claim: &QuarantineRecheckClaim,
        receipt: &ObservationReceipt,
    ) -> ConsignmentFuture<'_, ()>;

    /// 观察不可用或冲突时保留核对态。
    fn reconcile(
        &self,
        claim: &QuarantineRecheckClaim,
        reason: DispositionReconcileReason,
    ) -> ConsignmentFuture<'_, ()>;
}

/// 单轮隔离重检结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineRecheckRound {
    /// 没有 due 工作。
    Idle,
    /// 已追加观察。
    Appended {
        /// 追加后的隔离版本。
        quarantine_version: u64,
        /// 观察 UUID。
        observation_id: Uuid,
    },
    /// 临时读取失败，稍后重试。
    Retried,
    /// 观察冲突或不可用，留核对。
    Reconciled,
    /// 租约或围栏失效。
    LeaseLost,
}

/// 隔离重检用例。
pub struct QuarantineRecheckService {
    commands: Arc<DepositCommandService>,
    work: Arc<dyn QuarantineRecheckStore>,
    reader: Arc<dyn QuarantineRecheckReader>,
}

impl QuarantineRecheckService {
    /// 注入写用例、工作仓储与只读读取器。
    pub fn new(
        commands: Arc<DepositCommandService>,
        work: Arc<dyn QuarantineRecheckStore>,
        reader: Arc<dyn QuarantineRecheckReader>,
    ) -> Self {
        Self { commands, work, reader }
    }

    /// 领取并重读一封隔离信；只追加观察，零资产动作。
    pub async fn run_once(
        &self,
        request: DispositionRoundRequest,
    ) -> Result<QuarantineRecheckRound, ConsignmentError> {
        let Some(claim) = self
            .work
            .claim_due(request.worker_id.clone(), request.now, request.lease_until)
            .await?
        else {
            return Ok(QuarantineRecheckRound::Idle);
        };
        if !claim.validate() || request.now >= claim.lease_until {
            return Ok(QuarantineRecheckRound::LeaseLost);
        }
        if self.work.check_current(&claim).await.is_err() {
            return Ok(QuarantineRecheckRound::LeaseLost);
        }
        let observation = match self.reader.reread(&claim, request.deadline).await? {
            Ok(observation) => observation,
            Err(QuarantineRecheckReadError::TemporaryUnavailable) => {
                self.work
                    .release_for_retry(
                        &claim,
                        DispositionRetryReason::TemporaryUnavailable,
                        disposition_retry_at(request.now, claim.attempt_count),
                    )
                    .await?;
                return Ok(QuarantineRecheckRound::Retried);
            }
            Err(QuarantineRecheckReadError::LeaseLost) => {
                return Ok(QuarantineRecheckRound::LeaseLost);
            }
            Err(QuarantineRecheckReadError::InvalidObservation) => {
                self.work.reconcile(&claim, DispositionReconcileReason::EvidenceConflict).await?;
                return Ok(QuarantineRecheckRound::Reconciled);
            }
        };
        if observation.remote_observation_id.is_empty()
            || observation.observation.observation_id.is_nil()
            || observation.observation.manifest_digest == [0; 32]
        {
            self.work.reconcile(&claim, DispositionReconcileReason::EvidenceConflict).await?;
            return Ok(QuarantineRecheckRound::Reconciled);
        }
        if self.work.check_current(&claim).await.is_err() {
            return Ok(QuarantineRecheckRound::LeaseLost);
        }
        let legacy_owner = observation.observation.legacy_auction_owner;
        let observation_digest = observation.observation.manifest_digest;
        // 不变量 5；裁定 12：append 端口没有 fence 参数，4C 必须在 append 事务内核对重检工作的
        // fence，不一致就拒绝。当前检查与追加之间若恰好过租，旧持有者最多追加一条有效观察
        // （隔离版本加一、零资产动作）；远端观察身份相同时由存储按
        // `(quarantine_id, remote_observation_id)` 去重。
        let receipt = match self
            .commands
            .append_quarantine_observation(
                claim.quarantine_id,
                observation.remote_observation_id,
                observation.observed_at,
                observation.observation,
                observation.evidence_ref,
            )
            .await
        {
            Ok(receipt) => receipt,
            Err(ConsignmentError::Business(_))
            | Err(ConsignmentError::CommandIdConflict)
            | Err(ConsignmentError::OutcomeUnknown) => {
                self.work.reconcile(&claim, DispositionReconcileReason::EvidenceConflict).await?;
                return Ok(QuarantineRecheckRound::Reconciled);
            }
            Err(ConsignmentError::Unavailable) => {
                self.work
                    .release_for_retry(
                        &claim,
                        DispositionRetryReason::StorageUnavailable,
                        disposition_retry_at(request.now, claim.attempt_count),
                    )
                    .await?;
                return Ok(QuarantineRecheckRound::Retried);
            }
            Err(ConsignmentError::LeaseLost) => {
                return Ok(QuarantineRecheckRound::LeaseLost);
            }
            Err(error) => return Err(error),
        };
        if receipt.quarantine_id != claim.quarantine_id
            || receipt.observation_id.is_nil()
            || receipt.digest == [0; 32]
            || receipt.quarantine_version < claim.quarantine_version
        {
            self.work.reconcile(&claim, DispositionReconcileReason::EvidenceConflict).await?;
            return Ok(QuarantineRecheckRound::Reconciled);
        }
        if let Some(owner) = legacy_owner {
            // 裁定：重检命中旧拍卖 owner 时由系统自动交还原 owner，零资产动作；交接失败
            // 保留 active 隔离并按持久退避重试，不把失败伪装成已解除。
            let kind = legacy_owner_release_kind(owner);
            let binding = AutoReleaseBinding {
                observation_digest,
                source_ref: format!("legacy-auction-owner:{}", owner.as_str()),
            };
            match self
                .commands
                .release_quarantine_auto(
                    claim.quarantine_id,
                    receipt.quarantine_version,
                    receipt.observation_id,
                    kind,
                    binding,
                )
                .await
            {
                Ok(_) => {}
                Err(ConsignmentError::LeaseLost) => {
                    return Ok(QuarantineRecheckRound::LeaseLost);
                }
                Err(error) => {
                    // 除租约丢失外的任何失败都保持 active 隔离并退避；错误链只投影为安全码。
                    tracing::debug!(
                        code = error.api_error().map_or("unavailable", |(_, code)| code),
                        "隔离自动交接失败，保留隔离并退避"
                    );
                    self.work
                        .release_for_retry(
                            &claim,
                            DispositionRetryReason::TemporaryUnavailable,
                            disposition_retry_at(request.now, claim.attempt_count),
                        )
                        .await?;
                    return Ok(QuarantineRecheckRound::Retried);
                }
            }
        }
        self.work.complete(&claim, &receipt).await?;
        Ok(QuarantineRecheckRound::Appended {
            quarantine_version: receipt.quarantine_version,
            observation_id: receipt.observation_id,
        })
    }
}

fn legacy_owner_release_kind(owner: LegacyAuctionOwner) -> ReleaseKind {
    if owner.is_return_case() {
        ReleaseKind::AuctionReturnCase
    } else {
        ReleaseKind::AuctionRecoveryReturn
    }
}
