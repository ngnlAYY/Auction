//! 寄售第 4 期用例：规范摘要、系统命令 UUIDv5 与端口命令组装。

use std::sync::Arc;

use auction_domain::CommandId;
use auction_domain::UserId;
use auction_domain::consignment::deposit::{
    EnvelopeObservation, EnvelopeRoute, LockedEnvelopeFacts, ReleaseKind, TrustedConsignmentRts,
    decide_envelope_with_origin,
};
use auction_domain::consignment::{FrozenSeason, ListingVersion};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::{
    AppendQuarantineObservationCommand, ApplyMailProofCommand, AutoReleaseBinding,
    CustodyControlStore, DecideEnvelopeCommand, DepositActor, DepositOperation, DispositionAction,
    DispositionStore, EnvelopeStore, ExpireReservationCommand, IssueReservationCommand,
    MailProofKind, ObservationReceipt, PostPermit, RecheckQuarantineCommand,
    ReleaseOperationsHoldCommand, ReleaseQuarantineCommand, ReleaseReceipt,
    ReservationInvalidation, ReservationReceipt, ReservationRetired, ReservationStore,
    SeasonCloseCommand, SeasonCloseReceipt, SeasonCloseTarget, SeasonNoticeCommand,
    SeasonNoticeKind, SeasonNoticeReceipt, SeasonStore, SetOperationsHoldCommand,
    StartDispositionWriteCommand, WithdrawDepositCommand, WithdrawDepositReceipt, WriteStart,
    WriteStartOutcome,
};
use crate::ports::consignment::{
    ConsignmentError, ListingDelistReason, validate_listing_client_command_id,
    validate_listing_request_id,
};
use crate::ports::core::RequestHash;
use crate::services::consignment::deposit::encoding::{
    canonical_append_observation, canonical_apply_mail_proof, canonical_decide_envelope,
    canonical_expire_reservation, canonical_issue_reservation, canonical_recheck_quarantine,
    canonical_release_hold, canonical_release_quarantine, canonical_season_close,
    canonical_season_notice, canonical_set_hold, canonical_start_disposition_write,
    canonical_withdraw, expire_business_key, mail_key_business_key, observation_business_key,
    proof_business_key, season_close_business_key, season_notice_business_key,
    start_write_business_key, system_command_id,
};

/// Admin 解除隔离的输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseQuarantineAdminInput {
    /// Admin 稳定引用。
    pub admin_actor: String,
    /// Admin UUIDv4。
    pub command_id: CommandId,
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 预期版本。
    pub expected_version: u64,
    /// 所引新观察。
    pub observation_id: Uuid,
    /// 拟定结果。
    pub proposed_kind: ReleaseKind,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// 资产起写输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartDispositionWriteInput {
    /// 最终决定 UUID。
    pub final_decision_id: Uuid,
    /// 资产动作。
    pub action: DispositionAction,
    /// 投递 UUID。
    pub delivery_id: Uuid,
    /// 当前 fence。
    pub fence: u64,
    /// 原工作 UUID。
    pub work_id: Uuid,
    /// attempt UUID。
    pub attempt_id: Uuid,
    /// claim token。
    pub claim_token: Uuid,
    /// 会话绑定摘要。
    pub session_binding_digest: [u8; 32],
    /// 写前冻结清单摘要。
    pub expected_manifest_digest: [u8; 32],
}

/// 邮件证明输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyMailProofInput {
    /// 原 attempt UUID。
    pub original_attempt_id: Uuid,
    /// 证明种类。
    pub proof_kind: MailProofKind,
    /// 最终决定 UUID。
    pub final_decision_id: Uuid,
    /// 原资产动作。
    pub action: DispositionAction,
    /// 原投递 UUID。
    pub delivery_id: Uuid,
    /// 原 fence。
    pub fence: u64,
    /// 证明观察 UUID。
    pub observation_id: Uuid,
    /// 验证器版本。
    pub validator_version: String,
    /// 证明摘要。
    pub proof_digest: [u8; 32],
}

/// 寄售收件与代管写用例。
pub struct DepositCommandService {
    reservations: Arc<dyn ReservationStore>,
    envelopes: Arc<dyn EnvelopeStore>,
    dispositions: Arc<dyn DispositionStore>,
    controls: Arc<dyn CustodyControlStore>,
    seasons: Arc<dyn SeasonStore>,
}

impl DepositCommandService {
    /// 注入五个窄端口。
    pub fn new(
        reservations: Arc<dyn ReservationStore>,
        envelopes: Arc<dyn EnvelopeStore>,
        dispositions: Arc<dyn DispositionStore>,
        controls: Arc<dyn CustodyControlStore>,
        seasons: Arc<dyn SeasonStore>,
    ) -> Self {
        Self { reservations, envelopes, dispositions, controls, seasons }
    }

    /// 本人签发或换发预约。新码与预约 ID 由存储在首次提交时生成。
    pub async fn issue_reservation(
        &self,
        seller_user_id: UserId,
        command_id: CommandId,
        listing_id: Uuid,
        expected_version: ListingVersion,
        request_id: Option<String>,
    ) -> Result<ReservationReceipt, ConsignmentError> {
        validate_listing_client_command_id(command_id)?;
        validate_listing_request_id(request_id.as_deref())?;
        let mut command = IssueReservationCommand {
            command_id,
            seller_user_id,
            listing_id,
            expected_version,
            request_hash: RequestHash::new([0; 32]),
            request_id,
        };
        command.request_hash = canonical_issue_reservation(&command).request_hash();
        self.reservations.issue_reservation(command).await
    }

    /// 软关闭后按原命令重放预约签发回执。
    ///
    /// 请求摘要仍由 `canonical_issue_reservation` 计算；存储只读账本，不新建命令。
    pub async fn replay_issue_reservation(
        &self,
        seller_user_id: UserId,
        command_id: CommandId,
        listing_id: Uuid,
        expected_version: ListingVersion,
        request_id: Option<String>,
    ) -> Result<Option<ReservationReceipt>, ConsignmentError> {
        validate_listing_client_command_id(command_id)?;
        validate_listing_request_id(request_id.as_deref())?;
        let mut command = IssueReservationCommand {
            command_id,
            seller_user_id,
            listing_id,
            expected_version,
            request_hash: RequestHash::new([0; 32]),
            request_id,
        };
        command.request_hash = canonical_issue_reservation(&command).request_hash();
        self.reservations
            .replay_issue_reservation(
                command.seller_user_id,
                command.command_id,
                command.listing_id,
                command.request_hash,
            )
            .await
    }

    /// Worker 或命令子事实退休预约；系统 ID 由预约、代与失效事实派生。
    pub async fn expire_reservation(
        &self,
        reservation_id: Uuid,
        generation: u64,
        invalidation: ReservationInvalidation,
    ) -> Result<ReservationRetired, ConsignmentError> {
        let command_id = system_command_id(
            DepositOperation::ExpireReservation,
            &expire_business_key(reservation_id, generation, invalidation),
        );
        let mut command = ExpireReservationCommand {
            command_id,
            reservation_id,
            generation,
            invalidation,
            request_hash: RequestHash::new([0; 32]),
        };
        command.request_hash = canonical_expire_reservation(&command).request_hash();
        self.reservations.expire_reservation(command).await
    }

    /// 首次整封决定。`Pending` 与旧拍卖 owner 不写寄售终局，调用方继续原路径。
    pub async fn decide_envelope(
        &self,
        facts: LockedEnvelopeFacts,
    ) -> Result<Option<crate::ports::consignment::EnvelopeReceipt>, ConsignmentError> {
        let Some(command) = Self::prepare_decide_envelope(facts, None)? else {
            return Ok(None);
        };
        let receipt = self.envelopes.decide_envelope(command).await?;
        if receipt.validate_shape().is_err() {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(Some(receipt))
    }

    /// 纯构造首次整封决定命令；规范编码与系统 UUIDv5 只在这里产生。
    ///
    /// 旧拍卖 owner、临时读取失败与已有决定不生成命令，调用方按原路径或恢复处理。`origin` 是预读按
    /// RTS 线索定位的寄售原轮，只用于把“疑似系统 RTS”改交退件接管；写端锁内复判来源（不变量 6）。
    pub fn prepare_decide_envelope(
        facts: LockedEnvelopeFacts,
        origin: Option<TrustedConsignmentRts>,
    ) -> Result<Option<DecideEnvelopeCommand>, ConsignmentError> {
        let route = decide_envelope_with_origin(&facts, origin);
        let decision = match route {
            EnvelopeRoute::LegacyAuction(_) | EnvelopeRoute::Pending(_) => return Ok(None),
            EnvelopeRoute::Consignment(decision) => decision,
        };
        let Some(mail_key) = facts.mail_key.clone() else {
            return Err(ConsignmentError::InvalidDepositRequest);
        };
        let command_id =
            system_command_id(DepositOperation::DecideEnvelope, &mail_key_business_key(&mail_key));
        let mut command = DecideEnvelopeCommand {
            command_id,
            request_hash: RequestHash::new([0; 32]),
            mail_key,
            facts,
            decision,
        };
        command.request_hash = canonical_decide_envelope(&command).request_hash();
        Ok(Some(command))
    }

    /// 提交一个已构造的整封决定命令；仅供 `plan` 之后需要恢复原动作的调用方使用。
    pub async fn commit_decide_envelope(
        &self,
        command: DecideEnvelopeCommand,
    ) -> Result<crate::ports::consignment::EnvelopeReceipt, ConsignmentError> {
        let receipt = self.envelopes.decide_envelope(command).await?;
        if receipt.validate_shape().is_err() {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(receipt)
    }

    /// 追加一次隔离观察；同一远端观察重放不增版本。
    pub async fn append_quarantine_observation(
        &self,
        quarantine_id: Uuid,
        remote_observation_id: String,
        observed_at: Timestamp,
        observation: EnvelopeObservation,
        evidence_ref: Option<String>,
    ) -> Result<ObservationReceipt, ConsignmentError> {
        let command_id = system_command_id(
            DepositOperation::AppendQuarantineObservation,
            &observation_business_key(quarantine_id, &remote_observation_id),
        );
        let mut command = AppendQuarantineObservationCommand {
            command_id,
            request_hash: RequestHash::new([0; 32]),
            quarantine_id,
            remote_observation_id,
            observed_at,
            observation,
            evidence_ref,
        };
        command.request_hash = canonical_append_observation(&command).request_hash();
        self.envelopes.append_quarantine_observation(command).await
    }

    /// Admin 排队重检。
    pub async fn recheck_quarantine(
        &self,
        admin_actor: String,
        command_id: CommandId,
        quarantine_id: Uuid,
        expected_version: u64,
        request_id: Option<String>,
    ) -> Result<crate::ports::consignment::RecheckReceipt, ConsignmentError> {
        validate_listing_client_command_id(command_id)?;
        validate_listing_request_id(request_id.as_deref())?;
        let mut command = RecheckQuarantineCommand {
            command_id,
            admin_actor,
            quarantine_id,
            expected_version,
            request_hash: RequestHash::new([0; 32]),
            request_id,
        };
        command.request_hash = canonical_recheck_quarantine(&command).request_hash();
        self.envelopes.recheck_quarantine(command).await
    }

    /// Admin 引用具体新观察解除隔离。
    pub async fn release_quarantine_admin(
        &self,
        input: ReleaseQuarantineAdminInput,
    ) -> Result<ReleaseReceipt, ConsignmentError> {
        validate_listing_client_command_id(input.command_id)?;
        validate_listing_request_id(input.request_id.as_deref())?;
        let mut command = ReleaseQuarantineCommand {
            command_id: input.command_id,
            actor: DepositActor::Admin(input.admin_actor),
            quarantine_id: input.quarantine_id,
            expected_version: input.expected_version,
            observation_id: input.observation_id,
            proposed_kind: input.proposed_kind,
            request_hash: RequestHash::new([0; 32]),
            request_id: input.request_id,
            auto_binding: None,
        };
        command.request_hash = canonical_release_quarantine(&command).request_hash();
        let receipt = self.envelopes.release_quarantine(command).await?;
        if receipt.validate_shape().is_err() {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(receipt)
    }

    /// 自动复判系统变体；命令 ID 由隔离与新观察派生。
    pub async fn release_quarantine_auto(
        &self,
        quarantine_id: Uuid,
        expected_version: u64,
        observation_id: Uuid,
        proposed_kind: ReleaseKind,
        binding: AutoReleaseBinding,
    ) -> Result<ReleaseReceipt, ConsignmentError> {
        let command_id = system_command_id(DepositOperation::ReleaseQuarantine, &{
            let mut key = quarantine_id.as_bytes().to_vec();
            key.extend_from_slice(observation_id.as_bytes());
            key
        });
        let mut command = ReleaseQuarantineCommand {
            command_id,
            actor: DepositActor::System,
            quarantine_id,
            expected_version,
            observation_id,
            proposed_kind,
            request_hash: RequestHash::new([0; 32]),
            request_id: None,
            auto_binding: Some(binding),
        };
        command.request_hash = canonical_release_quarantine(&command).request_hash();
        let receipt = self.envelopes.release_quarantine(command).await?;
        if receipt.validate_shape().is_err() {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(receipt)
    }

    /// 提交一次资产起写；只有首次提交返回许可。
    pub async fn start_disposition_write(
        &self,
        input: StartDispositionWriteInput,
    ) -> Result<crate::ports::consignment::WriteStart, ConsignmentError> {
        let command_id = system_command_id(
            DepositOperation::StartDispositionWrite,
            &start_write_business_key(
                input.final_decision_id,
                input.action,
                input.delivery_id,
                input.fence,
            ),
        );
        let mut command = StartDispositionWriteCommand {
            command_id,
            request_hash: RequestHash::new([0; 32]),
            final_decision_id: input.final_decision_id,
            action: input.action,
            delivery_id: input.delivery_id,
            fence: input.fence,
            work_id: input.work_id,
            attempt_id: input.attempt_id,
            claim_token: input.claim_token,
            session_binding_digest: input.session_binding_digest,
            expected_manifest_digest: input.expected_manifest_digest,
        };
        command.request_hash = canonical_start_disposition_write(&command).request_hash();
        let delivery_id = command.delivery_id;
        let outcome = self.dispositions.start_disposition_write(command).await?;
        Ok(match outcome {
            WriteStartOutcome::FirstCommitted(receipt) => {
                let permit = PostPermit::new(receipt.work_id, delivery_id, receipt.fence);
                WriteStart::FirstCommitted { receipt, permit }
            }
            WriteStartOutcome::AlreadyCommitted(receipt) => WriteStart::AlreadyCommitted(receipt),
        })
    }

    /// 应用原 attempt 的证明；取得全封后存储逐件应用挂牌结果。
    pub async fn apply_mail_proof(
        &self,
        input: ApplyMailProofInput,
    ) -> Result<crate::ports::consignment::ProofReceipt, ConsignmentError> {
        let command_id = system_command_id(
            DepositOperation::ApplyMailProof,
            &proof_business_key(input.original_attempt_id, input.proof_kind),
        );
        let mut command = ApplyMailProofCommand {
            command_id,
            request_hash: RequestHash::new([0; 32]),
            original_attempt_id: input.original_attempt_id,
            proof_kind: input.proof_kind,
            final_decision_id: input.final_decision_id,
            action: input.action,
            delivery_id: input.delivery_id,
            fence: input.fence,
            observation_id: input.observation_id,
            validator_version: input.validator_version,
            proof_digest: input.proof_digest,
        };
        command.request_hash = canonical_apply_mail_proof(&command).request_hash();
        self.dispositions.apply_mail_proof(command).await
    }

    /// 卖家登记撤回意愿。
    pub async fn request_withdrawal(
        &self,
        seller_user_id: UserId,
        command_id: CommandId,
        listing_id: Uuid,
        expected_version: ListingVersion,
        request_id: Option<String>,
    ) -> Result<WithdrawDepositReceipt, ConsignmentError> {
        validate_listing_client_command_id(command_id)?;
        validate_listing_request_id(request_id.as_deref())?;
        let mut command = WithdrawDepositCommand {
            command_id,
            seller_user_id,
            listing_id,
            expected_version,
            request_hash: RequestHash::new([0; 32]),
            request_id,
        };
        command.request_hash = canonical_withdraw(&command).request_hash();
        let receipt = self.controls.request_withdrawal(command).await?;
        if receipt.validate_shape().is_err() {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(receipt)
    }

    /// Admin 下架 v2：锁内按状态关闭未代管件或对代管件设独立止售。
    pub async fn set_operations_hold(
        &self,
        admin_actor: String,
        command_id: CommandId,
        listing_id: Uuid,
        expected_version: ListingVersion,
        reason: ListingDelistReason,
        request_id: Option<String>,
    ) -> Result<crate::ports::consignment::OperationsHoldReceipt, ConsignmentError> {
        validate_listing_client_command_id(command_id)?;
        validate_listing_request_id(request_id.as_deref())?;
        let mut command = SetOperationsHoldCommand {
            admin_actor,
            command_id,
            listing_id,
            expected_version,
            request_hash: RequestHash::new([0; 32]),
            request_id,
            reason,
        };
        command.request_hash = canonical_set_hold(&command).request_hash();
        self.controls.set_operations_hold(command).await
    }

    /// 软关闭后按原命令重放 Admin 下架或止售回执。
    ///
    /// 请求摘要仍由 `canonical_set_hold` 计算；存储只读账本，不新建命令。
    pub async fn replay_set_operations_hold(
        &self,
        admin_actor: String,
        command_id: CommandId,
        listing_id: Uuid,
        expected_version: ListingVersion,
        reason: ListingDelistReason,
        request_id: Option<String>,
    ) -> Result<Option<crate::ports::consignment::OperationsHoldReceipt>, ConsignmentError> {
        validate_listing_client_command_id(command_id)?;
        validate_listing_request_id(request_id.as_deref())?;
        let mut command = SetOperationsHoldCommand {
            admin_actor,
            command_id,
            listing_id,
            expected_version,
            request_hash: RequestHash::new([0; 32]),
            request_id,
            reason,
        };
        command.request_hash = canonical_set_hold(&command).request_hash();
        self.controls
            .replay_set_operations_hold(
                command.admin_actor,
                command.command_id,
                command.listing_id,
                command.request_hash,
            )
            .await
    }

    /// Admin 解除指定运营止售。
    pub async fn release_operations_hold(
        &self,
        admin_actor: String,
        command_id: CommandId,
        listing_id: Uuid,
        expected_version: ListingVersion,
        hold_id: Uuid,
        request_id: Option<String>,
    ) -> Result<crate::ports::consignment::OperationsHoldReceipt, ConsignmentError> {
        validate_listing_client_command_id(command_id)?;
        validate_listing_request_id(request_id.as_deref())?;
        let mut command = ReleaseOperationsHoldCommand {
            admin_actor,
            command_id,
            listing_id,
            expected_version,
            hold_id,
            request_hash: RequestHash::new([0; 32]),
            request_id,
        };
        command.request_hash = canonical_release_hold(&command).request_hash();
        self.controls.release_operations_hold(command).await
    }

    /// 预停后按 `(season,position,seller)` 去重地排队撤回通知。
    pub async fn queue_season_notice(
        &self,
        season: FrozenSeason,
        position_id: Uuid,
        seller_user_id: UserId,
        notice_kind: SeasonNoticeKind,
    ) -> Result<SeasonNoticeReceipt, ConsignmentError> {
        let command_id = system_command_id(
            DepositOperation::SeasonNotice,
            &season_notice_business_key(&season.season_id, position_id, seller_user_id),
        );
        let mut command = SeasonNoticeCommand {
            command_id,
            season,
            position_id,
            seller_user_id,
            notice_kind,
            request_hash: RequestHash::new([0; 32]),
        };
        command.request_hash = canonical_season_notice(&command).request_hash();
        self.seasons.queue_season_notice(command).await
    }

    /// 关闭旧季目标；冻结界不能因迟跑而延长。
    pub async fn close_season(
        &self,
        season: FrozenSeason,
        target: SeasonCloseTarget,
    ) -> Result<SeasonCloseReceipt, ConsignmentError> {
        let command_id = system_command_id(
            DepositOperation::SeasonClose,
            &season_close_business_key(&season.season_id, target),
        );
        let mut command = SeasonCloseCommand {
            command_id,
            season,
            target,
            request_hash: RequestHash::new([0; 32]),
        };
        command.request_hash = canonical_season_close(&command).request_hash();
        self.seasons.close_season(command).await
    }
}
