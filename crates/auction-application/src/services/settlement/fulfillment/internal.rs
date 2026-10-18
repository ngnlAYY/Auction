//! 新 pending 的真实身份分支；opaque 句柄覆盖整个提交，不跨任务释放独占会话。

use super::{SettlementFulfillmentService, validation::repository_error};
use crate::{
    ApplicationError, InternalEquipmentTarget, InternalEvidencePurpose, InternalEvidenceRequest,
    SettlementFulfillmentClaim, SettlementFulfillmentPhaseResult,
    SettlementFulfillmentPreparationOutcome as Outcome, SettlementFulfillmentState,
    SettlementInternalCommand, SettlementInternalCommitOutcome, SettlementInternalEvidenceReader,
    SettlementInternalFulfillmentStore,
};
use auction_domain::InternalSettlementAction;
use chrono::{DateTime, Duration, Utc};
use std::{sync::Arc, time::Instant};

impl SettlementFulfillmentService {
    /// 必须注入原 MoogleMailGateway 与数据库；未装配仅延期，不以缺能力推断非 bot。
    pub fn with_internal_fulfillment(
        mut self,
        store: Arc<dyn SettlementInternalFulfillmentStore>,
        reader: Arc<dyn SettlementInternalEvidenceReader>,
    ) -> Self {
        self.internal = Some((store, reader));
        self
    }

    pub(super) async fn prepare_classified(
        &self,
        mut claim: SettlementFulfillmentClaim,
        prepared_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> Result<Outcome, ApplicationError> {
        super::validation::validate_claim(&claim)?;
        if claim.state != SettlementFulfillmentState::Pending {
            return Err(repository_error("仅新 pending 可进行内部身份分支"));
        }
        let Some((store, reader)) = &self.internal else {
            return self.defer_internal(claim, retry_delay, "internal_unsupported").await;
        };
        // 稳定命令先重放，不要求重试仍持原 claim/远端会话。
        let replay = SettlementInternalCommand::new(claim.clone(), None, false)?;
        if store.replay_internal(replay).await? {
            return Ok(Outcome::InternallySatisfied { obligation_id: claim.obligation_id });
        }
        // 内部动作也遵守原义务的世界/金额/来源闭集，不绕过canonical intent校验。
        super::intent::build_delivery_intent(&claim)?;
        if claim.delivery_id.is_some()
            || claim.prepare_command_id.is_some()
            || claim.activate_command_id.is_some()
        {
            return self.defer_internal(claim, retry_delay, "internal_history_conflict").await;
        }
        let recipient_version = if let Some(user) = claim.recipient_user_id {
            match self.recipient_reader.read_recipient(user).await {
                Ok(current)
                    if current.user_id == user && current.eligible && current.state_version > 0 =>
                {
                    claim.recipient = current.game_identity;
                    Some(current.state_version)
                }
                _ => return self.defer_internal(claim, retry_delay, "recipient_unavailable").await,
            }
        } else {
            None
        };
        let command = SettlementInternalCommand::new(claim.clone(), recipient_version, false)?;
        let request = match evidence_request(&command, self.clock.now(), false, false) {
            Ok(request) => request,
            Err(_) => {
                return self.defer_internal(claim, retry_delay, "internal_evidence_wait").await;
            }
        };
        let deadline = request.deadline;
        let identity = match reader.prepare_internal_evidence(request).await {
            Ok(identity) => identity,
            Err(_) => {
                return self.defer_internal(claim, retry_delay, "internal_evidence_wait").await;
            }
        };
        // Admin 明确配置的 bot 名称与页面身份等值处理；其余情况仍要求远端身份精确匹配。
        if identity.snapshot().identity.player_name != claim.recipient
            && !self.is_configured_bot(&claim.recipient)
        {
            if identity.check_current().await.is_err() {
                drop(identity);
                return self.defer_internal(claim, retry_delay, "internal_evidence_wait").await;
            }
            // 原外发授权还会读取余额，先释放非重入guard；发送owner仍须独立核验当前凭据。
            drop(identity);
            if claim.kind == auction_domain::SettlementObligationKind::BuyerRefund {
                return self
                    .prepare_funded_refund(
                        claim,
                        recipient_version,
                        prepared_at,
                        retry_delay,
                        deadline,
                    )
                    .await;
            }
            return self
                .prepare_external(claim, prepared_at, retry_delay)
                .await
                .map(Box::new)
                .map(Outcome::External);
        }
        let Some(action) = InternalSettlementAction::for_kind(claim.kind) else {
            drop(identity);
            return self.defer_internal(claim, retry_delay, "internal_return_unsupported").await;
        };
        // 第二次组合读取必须仍使用第一次开始时的绝对预算，不重置30秒。
        drop(identity);
        let mut request = evidence_request(
            &command,
            self.clock.now(),
            action.requires_equipment(),
            action.requires_funding(),
        )?;
        request.deadline = request.deadline.min(deadline);
        let mut prepared = match reader.prepare_internal_evidence(request).await {
            Ok(prepared) => prepared,
            Err(_) => {
                return self.defer_internal(claim, retry_delay, "internal_evidence_wait").await;
            }
        };
        if prepared.snapshot().identity.player_name != claim.recipient
            || prepared.check_current().await.is_err()
        {
            return self.defer_internal(claim, retry_delay, "internal_evidence_wait").await;
        }
        let guard = match prepared.begin_commit() {
            Ok(guard) => guard,
            Err(_) => {
                return self.defer_internal(claim, retry_delay, "internal_evidence_wait").await;
            }
        };
        match store.satisfy_internal(&command, guard.snapshot()).await {
            Ok(SettlementInternalCommitOutcome::Committed) => {
                guard.confirm_committed();
                Ok(Outcome::InternallySatisfied { obligation_id: claim.obligation_id })
            }
            Ok(SettlementInternalCommitOutcome::FundingDeferred) => {
                guard.confirm_committed();
                self.defer_internal(claim, retry_delay, "internal_funds_wait").await
            }
            Ok(SettlementInternalCommitOutcome::RolledBack(_)) => {
                guard.confirm_rolled_back();
                self.defer_internal(claim, retry_delay, "internal_proof_or_funds_wait").await
            }
            Err(_) => {
                // 未确认guard在返回时撤销owner；不改claim，不换命令，不转外发。
                Err(repository_error("内部履约提交结果未知，须查询原稳定命令"))
            }
        }
    }

    pub(super) async fn defer_internal(
        &self,
        claim: SettlementFulfillmentClaim,
        retry_delay: Duration,
        code: &'static str,
    ) -> Result<Outcome, ApplicationError> {
        let obligation_id = claim.obligation_id;
        self.store
            .fail(claim, retry_delay, code.to_owned(), "内部证据、来源或资金等待核验".to_owned())
            .await?;
        Ok(Outcome::External(Box::new(SettlementFulfillmentPhaseResult::Deferred {
            obligation_id,
            failure_code: code,
        })))
    }
}

pub(super) fn evidence_request(
    command: &SettlementInternalCommand,
    now: DateTime<Utc>,
    equipment: bool,
    cash: bool,
) -> Result<InternalEvidenceRequest, ApplicationError> {
    let claim = &command.claim;
    let budget = (claim.lease_until - now)
        .to_std()
        .map_err(|_| repository_error("履约租约已过期"))?
        .min(std::time::Duration::from_secs(30));
    if budget <= std::time::Duration::from_secs(1) {
        return Err(repository_error("履约证据预算不足"));
    }
    Ok(InternalEvidenceRequest {
        operation_id: command.command_id,
        purpose: InternalEvidencePurpose::SettlementClaim {
            obligation_id: claim.obligation_id.as_uuid(),
            claim_token: claim.claim_token,
            claim_fencing: claim.claim_fencing,
            state_version: claim.state_version,
        },
        world: claim.world,
        equipment: equipment.then(|| InternalEquipmentTarget {
            equipment_id: claim.equipment_id,
            external_equipment_id: claim.external_equipment_id.clone(),
            world: claim.equipment_world,
        }),
        observe_main_cash: cash,
        deadline: Instant::now() + budget,
    })
}
