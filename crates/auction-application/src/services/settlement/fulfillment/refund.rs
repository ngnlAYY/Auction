//! 原合法退款的外部资金用途接缝；不产生新退款权利、不替代原prepare的授权。

use chrono::{DateTime, Duration, Utc};

use super::{
    SettlementFulfillmentService, internal::evidence_request, validation::repository_error,
};
use crate::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, SafeErrorKind, diagnostic_id, record_error,
};
use crate::{
    ApplicationError, SettlementFulfillmentClaim,
    SettlementFulfillmentPreparationOutcome as Outcome, SettlementInternalCommand,
    SettlementInternalCommitOutcome,
};

impl SettlementFulfillmentService {
    #[tracing::instrument(
        skip_all,
        fields(
            obligation_id = %claim.obligation_id,
        )
    )]
    pub(super) async fn prepare_funded_refund(
        &self,
        claim: SettlementFulfillmentClaim,
        recipient_version: Option<u64>,
        prepared_at: DateTime<Utc>,
        retry_delay: Duration,
        deadline: std::time::Instant,
    ) -> Result<Outcome, ApplicationError> {
        let Some((store, reader)) = &self.internal else {
            return self.defer_internal(claim, retry_delay, "internal_unsupported").await;
        };
        let command =
            match SettlementInternalCommand::external_refund(claim.clone(), recipient_version) {
                Ok(command) => command,
                Err(_) => return self.manual_internal(claim, "invalid_fulfillment_fact").await,
            };
        let replayed = match store.replay_internal(command.clone()).await {
            Ok(replayed) => replayed,
            Err(ApplicationError::FulfillmentDependencyMismatch) => {
                // 原负载无法精确重建时隔离；不能把冲突当未执行，再次预留退款用途。
                return self.manual_internal(claim, "delivery_conflict").await;
            }
            Err(error) => return Err(error),
        };
        if replayed {
            return self
                .prepare_external(claim, prepared_at, retry_delay)
                .await
                .map(Box::new)
                .map(Outcome::External);
        }
        let policy = self.funding_policy().await?;
        let mut request =
            match evidence_request(&command, self.clock.now(), true, policy.observes_cash()) {
                Ok(request) => request,
                Err(error) => {
                    record_error(
                        DiagnosticLevel::Warn,
                        error.diagnostic_kind(),
                        "fulfillment_refund",
                        "build_evidence_request",
                        DiagnosticContext {
                            obligation_id: diagnostic_id(&claim.obligation_id),
                            world: Some(claim.world),
                            ..Default::default()
                        },
                    );

                    return self
                        .defer_internal(claim, retry_delay, "refund_funding_evidence_wait")
                        .await;
                }
            };
        request.deadline = request.deadline.min(deadline);
        let mut prepared = match reader.prepare_internal_evidence(request).await {
            Ok(prepared) => prepared,
            Err(error) => {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "fulfillment_refund",
                    "read_evidence",
                    DiagnosticContext {
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        ..Default::default()
                    },
                );

                return self
                    .defer_internal(claim, retry_delay, "refund_funding_evidence_wait")
                    .await;
            }
        };
        if !policy.matches_identity(&prepared.snapshot().identity.authenticated_uid)
            || prepared
                .check_current()
                .await
                .inspect_err(|error| {
                    record_error(
                        DiagnosticLevel::Warn,
                        error.diagnostic_kind(),
                        "fulfillment_refund",
                        "validate_evidence_freshness",
                        DiagnosticContext {
                            obligation_id: diagnostic_id(&claim.obligation_id),
                            world: Some(claim.world),
                            ..Default::default()
                        },
                    );
                })
                .is_err()
        {
            record_error(
                DiagnosticLevel::Warn,
                SafeErrorKind::DEPENDENCY_MISMATCH,
                "fulfillment_refund",
                "validate_evidence_binding",
                DiagnosticContext {
                    obligation_id: diagnostic_id(&claim.obligation_id),
                    world: Some(claim.world),
                    defer_reason: Some("refund_funding_evidence_wait"),
                    ..Default::default()
                },
            );
            return self.defer_internal(claim, retry_delay, "refund_funding_evidence_wait").await;
        }
        let guard = match prepared.begin_commit() {
            Ok(guard) => guard,
            Err(error) => {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "fulfillment_refund",
                    "acquire_commit_guard",
                    DiagnosticContext {
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        ..Default::default()
                    },
                );

                return self
                    .defer_internal(claim, retry_delay, "refund_funding_evidence_wait")
                    .await;
            }
        };
        let committed = match store.prepare_external_refund(&command, guard.snapshot()).await {
            Ok(SettlementInternalCommitOutcome::Committed) => {
                guard.confirm_committed();
                true
            }
            Ok(SettlementInternalCommitOutcome::FundingDeferred) => {
                record_error(
                    DiagnosticLevel::Info,
                    SafeErrorKind::FUNDING_UNAVAILABLE,
                    "fulfillment_refund",
                    "defer_funding",
                    DiagnosticContext {
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        defer_reason: Some("refund_funding_wait"),
                        ..Default::default()
                    },
                );
                guard.confirm_committed();
                false
            }
            Ok(SettlementInternalCommitOutcome::RolledBack(error)) => {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "fulfillment_refund",
                    "rollback_funding",
                    DiagnosticContext {
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        ..Default::default()
                    },
                );
                guard.confirm_rolled_back();
                false
            }
            Err(error) => {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "fulfillment_refund",
                    "commit_funding",
                    DiagnosticContext {
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        ..Default::default()
                    },
                );
                return Err(repository_error("退款用途提交结果未知，须查询原稳定命令"));
            }
        };
        // 下一阶段旧余额/custody读取会取得原非重入guard；先确定事务并释放本次组合证据。
        drop(prepared);
        if committed {
            self.prepare_external(claim, prepared_at, retry_delay)
                .await
                .map(Box::new)
                .map(Outcome::External)
        } else {
            self.defer_internal(claim, retry_delay, "refund_funding_wait").await
        }
    }
}
