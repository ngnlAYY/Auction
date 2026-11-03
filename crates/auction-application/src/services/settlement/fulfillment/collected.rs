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

const DIAGNOSTICS_TARGET: &str = "auction::settlement::funding_diagnostics";

impl SettlementFulfillmentService {
    #[tracing::instrument(
        target = "auction::settlement::funding_diagnostics",
        level = "warn",
        name = "collected_funding",
        skip_all,
        fields(
            obligation_id = %claim.obligation_id,
            delivery_id = ?claim.delivery_id,
            world = claim.world.as_str(),
            command_id
        )
    )]
    pub(super) async fn reconcile_collected(
        &self,
        claim: SettlementFulfillmentClaim,
        proof: SettlementFulfillmentProof,
        retry_delay: Duration,
    ) -> Result<SettlementFulfillmentReconcileResult, ApplicationError> {
        let delivery_id = claim.delivery_id.ok_or_else(|| {
            let error = repository_error("实收缺少原delivery");
            diagnose("collected_delivery_missing", Some(error.safe_kind()), "delivery", None);
            error
        })?;
        let result = |status| SettlementFulfillmentReconcileResult {
            obligation_id: claim.obligation_id,
            delivery_id,
            status,
        };
        let Some((store, reader)) = &self.internal else {
            // 旧入口的数据库门会在任何受保护账户存在时拒绝，未启用账户仍保留原外部语义。
            return self.store.satisfy(claim, proof).await.inspect_err(|error| {
                diagnose(
                    "collected_external_satisfy_failed",
                    Some(error.safe_kind()),
                    "external_satisfy",
                    None,
                );
            });
        };
        let command =
            SettlementInternalCommand::new(claim.clone(), None, true).inspect_err(|error| {
                diagnose("collected_command_invalid", Some(error.safe_kind()), "command", None);
            })?;
        {
            let span = tracing::Span::current();
            // target 关闭时当前 span 可能是祖先；只补本诊断 span，避免改写其他日志上下文。
            if span.metadata().is_some_and(|metadata| {
                metadata.target() == DIAGNOSTICS_TARGET && metadata.name() == "collected_funding"
            }) {
                span.record("command_id", tracing::field::display(command.command_id));
            }
        }
        if store.replay_internal(command.clone()).await.inspect_err(|error| {
            diagnose("collected_replay_failed", Some(error.safe_kind()), "replay", None);
        })? {
            return Ok(result(SettlementFulfillmentReconcileStatus::Satisfied));
        }
        let policy = self.funding_policy().await.inspect_err(|error| {
            diagnose("collected_policy_read_failed", Some(error.safe_kind()), "policy", None);
        })?;
        let request = evidence_request(&command, self.clock.now(), false, policy.observes_cash())
            .inspect_err(|error| {
            diagnose(
                "collected_evidence_request_failed",
                Some(error.safe_kind()),
                "evidence_request",
                None,
            );
        })?;
        let target = InternalSentCollectionTarget {
            external_mail_id: proof.reference.clone(),
            world: claim.world,
            recipient: claim.recipient.clone(),
            subject: canonical_settlement_fulfillment_subject(claim.kind, claim.obligation_id)
                .ok_or_else(|| {
                    let error = repository_error("实收主题无效");
                    diagnose("collected_subject_invalid", Some(error.safe_kind()), "subject", None);
                    error
                })?,
            // 旧成功metadata没有可信sent_at，不以sending_at或now冒充预期。
            expected_sent_at: None,
        };
        let mut prepared = match reader.prepare_internal_evidence_with_sent(request, target).await {
            Ok(prepared) => prepared,
            Err(error) => {
                diagnose(
                    "collected_evidence_prepare_failed",
                    Some(error.safe_kind()),
                    "evidence_prepare",
                    None,
                );
                // 未启用资金账户时保留原外部满足语义；数据库scope门保证受保护账户绝不走此旁路。
                // 原Sent证明已经通过旧精确payload校验，这里不生成任何资金来源或UID归属。
                match self.store.satisfy(claim.clone(), proof.clone()).await {
                    Ok(legacy) => return Ok(legacy),
                    Err(error) => diagnose(
                        "collected_legacy_satisfy_failed",
                        Some(error.safe_kind()),
                        "legacy_satisfy",
                        None,
                    ),
                }
                let deferred = result(SettlementFulfillmentReconcileStatus::Deferred);
                self.defer_internal(claim, retry_delay, "collected_funding_evidence_wait")
                    .await
                    .inspect_err(|error| {
                        diagnose_defer(error, "evidence_prepare", "collected_funding_evidence_wait")
                    })?;
                return Ok(deferred);
            }
        };
        // 身份拒绝仍短路，不触发 check_current；只增加诊断，不改变证据与资金判定。
        let evidence_current =
            if !policy.matches_identity(&prepared.snapshot().identity.authenticated_uid) {
                diagnose("collected_identity_mismatch", None, "identity", None);
                false
            } else if let Err(error) = prepared.check_current().await {
                diagnose(
                    "collected_evidence_current_failed",
                    Some(error.safe_kind()),
                    "evidence_current",
                    None,
                );
                false
            } else {
                true
            };
        if !evidence_current {
            let deferred = result(SettlementFulfillmentReconcileStatus::Deferred);
            self.defer_internal(claim, retry_delay, "collected_funding_evidence_wait")
                .await
                .inspect_err(|error| {
                    diagnose_defer(error, "evidence_current", "collected_funding_evidence_wait")
                })?;
            return Ok(deferred);
        }
        let guard = match prepared.begin_commit() {
            Ok(guard) => guard,
            Err(error) => {
                diagnose(
                    "collected_commit_guard_failed",
                    Some(error.safe_kind()),
                    "commit_guard",
                    None,
                );
                let deferred = result(SettlementFulfillmentReconcileStatus::Deferred);
                self.defer_internal(claim, retry_delay, "collected_funding_evidence_wait")
                    .await
                    .inspect_err(|error| {
                        diagnose_defer(error, "commit_guard", "collected_funding_evidence_wait")
                    })?;
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
                tracing::warn!(
                    target: DIAGNOSTICS_TARGET,
                    safe_kind = "collected_funding_deferred",
                    stage = "commit",
                    outcome = "committed",
                    defer_reason = "collected_funding_wait",
                    "COD 资金门延后已确认提交"
                );
                let deferred = result(SettlementFulfillmentReconcileStatus::Deferred);
                self.defer_internal(claim, retry_delay, "collected_funding_wait")
                    .await
                    .inspect_err(|error| {
                        diagnose_defer(error, "funding_deferred", "collected_funding_wait")
                    })?;
                Ok(deferred)
            }
            Ok(SettlementInternalCommitOutcome::RolledBack(error)) => {
                guard.confirm_rolled_back();
                tracing::warn!(
                    target: DIAGNOSTICS_TARGET,
                    safe_kind = "collected_funding_rolled_back",
                    cause_kind = error.safe_kind(),
                    stage = "commit",
                    outcome = "rolled_back",
                    defer_reason = "collected_funding_wait",
                    "COD 资金化拒绝已确认回滚"
                );
                let deferred = result(SettlementFulfillmentReconcileStatus::Deferred);
                self.defer_internal(claim, retry_delay, "collected_funding_wait")
                    .await
                    .inspect_err(|error| {
                        diagnose_defer(error, "funding_rolled_back", "collected_funding_wait")
                    })?;
                Ok(deferred)
            }
            Err(error) => {
                // 不确认 guard：结果未知仍撤销原 owner，保留原稳定命令与人工证明事实。
                tracing::warn!(
                    target: DIAGNOSTICS_TARGET,
                    safe_kind = "collected_funding_outcome_unconfirmed",
                    cause_kind = error.safe_kind(),
                    stage = "commit",
                    outcome = "unconfirmed",
                    "COD 实收事务结果尚未确认"
                );
                Err(repository_error("实收事务结果未知，须查询原稳定命令"))
            }
        }
    }
}

fn diagnose(
    safe_kind: &'static str,
    cause_kind: Option<&'static str>,
    stage: &'static str,
    defer_reason: Option<&'static str>,
) {
    tracing::warn!(
        target: DIAGNOSTICS_TARGET,
        safe_kind,
        cause_kind,
        stage,
        defer_reason,
        "COD 实收资金化尚未通过"
    );
}

fn diagnose_defer(error: &ApplicationError, stage: &'static str, defer_reason: &'static str) {
    diagnose("collected_defer_persist_failed", Some(error.safe_kind()), stage, Some(defer_reason));
}
