//! 外部 COD 实收同 session 重核与资金化；保持原 Sent collected 证明门。

use super::{
    SettlementFulfillmentService, internal::evidence_request, validation::repository_error,
};
use crate::{
    ApplicationError, InternalSentCollectionTarget, SettlementFulfillmentClaim,
    SettlementFulfillmentProof, SettlementFulfillmentReconcileResult,
    SettlementFulfillmentReconcileStatus, SettlementInternalCommand,
    SettlementInternalCommitOutcome, canonical_settlement_fulfillment_subject,
};
use chrono::Duration;

impl SettlementFulfillmentService {
    pub(super) async fn reconcile_collected(
        &self,
        claim: SettlementFulfillmentClaim,
        proof: SettlementFulfillmentProof,
        retry_delay: Duration,
    ) -> Result<SettlementFulfillmentReconcileResult, ApplicationError> {
        let delivery_id =
            claim.delivery_id.ok_or_else(|| repository_error("实收缺少原delivery"))?;
        let result = |status| SettlementFulfillmentReconcileResult {
            obligation_id: claim.obligation_id,
            delivery_id,
            status,
        };
        let Some((store, reader)) = &self.internal else {
            // 旧入口的数据库门会在任何受保护账户存在时拒绝，未启用账户仍保留原外部语义。
            return self.store.satisfy(claim, proof).await;
        };
        let command = SettlementInternalCommand::new(claim.clone(), None, true)?;
        if store.replay_internal(command.clone()).await? {
            return Ok(result(SettlementFulfillmentReconcileStatus::Satisfied));
        }
        let policy = self.funding_policy().await?;
        let request = evidence_request(&command, self.clock.now(), false, policy.observes_cash())?;
        let target = InternalSentCollectionTarget {
            external_mail_id: proof.reference.clone(),
            world: claim.world,
            recipient: claim.recipient.clone(),
            subject: canonical_settlement_fulfillment_subject(claim.kind, claim.obligation_id)
                .ok_or_else(|| repository_error("实收主题无效"))?,
            // 旧成功metadata没有可信sent_at，不以sending_at或now冒充预期。
            expected_sent_at: None,
        };
        let mut prepared = match reader.prepare_internal_evidence_with_sent(request, target).await {
            Ok(prepared) => prepared,
            Err(_) => {
                // 未启用资金账户时保留原外部满足语义；数据库scope门保证受保护账户绝不走此旁路。
                // 原Sent证明已经通过旧精确payload校验，这里不生成任何资金来源或UID归属。
                if let Ok(legacy) = self.store.satisfy(claim.clone(), proof.clone()).await {
                    return Ok(legacy);
                }
                let deferred = result(SettlementFulfillmentReconcileStatus::Deferred);
                self.defer_internal(claim, retry_delay, "collected_funding_evidence_wait").await?;
                return Ok(deferred);
            }
        };
        if !policy.matches_identity(&prepared.snapshot().identity.authenticated_uid)
            || prepared.check_current().await.is_err()
        {
            let deferred = result(SettlementFulfillmentReconcileStatus::Deferred);
            self.defer_internal(claim, retry_delay, "collected_funding_evidence_wait").await?;
            return Ok(deferred);
        }
        let guard = match prepared.begin_commit() {
            Ok(guard) => guard,
            Err(_) => {
                let deferred = result(SettlementFulfillmentReconcileStatus::Deferred);
                self.defer_internal(claim, retry_delay, "collected_funding_evidence_wait").await?;
                return Ok(deferred);
            }
        };
        match store.satisfy_collected(&command, &proof, guard.snapshot()).await {
            Ok(SettlementInternalCommitOutcome::Committed) => {
                guard.confirm_committed();
                Ok(result(SettlementFulfillmentReconcileStatus::Satisfied))
            }
            Ok(SettlementInternalCommitOutcome::FundingDeferred) => {
                guard.confirm_committed();
                let deferred = result(SettlementFulfillmentReconcileStatus::Deferred);
                self.defer_internal(claim, retry_delay, "collected_funding_wait").await?;
                Ok(deferred)
            }
            Ok(SettlementInternalCommitOutcome::RolledBack(_)) => {
                guard.confirm_rolled_back();
                let deferred = result(SettlementFulfillmentReconcileStatus::Deferred);
                self.defer_internal(claim, retry_delay, "collected_funding_wait").await?;
                Ok(deferred)
            }
            Err(_) => Err(repository_error("实收事务结果未知，须查询原稳定命令")),
        }
    }
}
