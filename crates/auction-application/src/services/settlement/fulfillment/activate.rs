//! 结算履约 `prepared` 阶段：激活 delivery 并把义务推进到 enqueued。

use chrono::{DateTime, Duration, Utc};

use super::authorization::authorize_pending_or_prepared;
use super::intent::{activation_request_hash, stable_activate_command_id};
use super::snapshot::delivery_snapshot_matches;
use super::validation::{repository_error, validate_claim};
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, diagnostic_id, record_error,
};
use crate::ports::settlement::{
    PreparedSettlementInternalEvidence, SettlementFulfillmentActivationResult,
    SettlementFulfillmentClaim, SettlementFulfillmentPhaseResult, SettlementFulfillmentState,
};
use crate::services::settlement::SettlementFulfillmentService;

impl SettlementFulfillmentService {
    #[tracing::instrument(

        skip_all,

        fields(

            obligation_id = %claim.obligation_id,

            delivery_id = ?claim.delivery_id,

        )

    )]
    pub(super) async fn activate_now(
        &self,
        claim: SettlementFulfillmentClaim,
        activated_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> Result<
        SettlementFulfillmentPhaseResult<SettlementFulfillmentActivationResult>,
        ApplicationError,
    > {
        validate_claim(&claim)?;
        if claim.state != SettlementFulfillmentState::Prepared {
            return Err(repository_error("当前履约阶段不允许激活 delivery"));
        }
        let (claim, intent, mut custody) = match authorize_pending_or_prepared(
            self.store.as_ref(),
            self.recipient_reader.as_ref(),
            self.custody_reader.as_ref(),
            self.evidence_reader.as_ref(),
            self.clock.as_ref(),
            claim,
            retry_delay,
            &self.funding_policy().await?,
        )
        .await?
        .into_phase()
        {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };
        let (Some(delivery_id), Some(_prepare_command_id), Some(prepare_request_hash)) =
            (claim.delivery_id, claim.prepare_command_id, claim.prepare_request_hash)
        else {
            return Err(repository_error("prepared 履约缺少 delivery prepare identity"));
        };
        if !delivery_snapshot_matches(&claim, &intent) {
            let obligation_id = claim.obligation_id;
            self.store.manual_reconcile(claim, "delivery_conflict".to_owned()).await?;
            return Ok(SettlementFulfillmentPhaseResult::ManualReconcile {
                obligation_id,
                failure_code: "delivery_conflict",
            });
        }
        let activation_command_id = stable_activate_command_id(claim.obligation_id);
        let activation_request_hash =
            activation_request_hash(claim.obligation_id, delivery_id, prepare_request_hash)?;
        // 激活继续复用当前 claim；持有租约时 next_attempt_at 必须为空。
        // 下方 reconcile 释放 claim 时再把 COD/货款对齐到共享追踪窗口。
        let guard = custody
            .as_mut()
            .map(PreparedSettlementInternalEvidence::begin_commit)
            .transpose()
            .map_err(|error| {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "fulfillment",
                    "acquire_activation_guard",
                    DiagnosticContext {
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        ..Default::default()
                    },
                );
                ApplicationError::FulfillmentDependencyUnavailable
            })?;
        let result = self
            .store
            .activate_delivery(
                claim,
                activation_command_id,
                activation_request_hash,
                activated_at,
                None,
                guard.as_ref().map(|guard| (guard.snapshot(), guard.deadline())),
            )
            .await?;
        if let Some(guard) = guard {
            guard.confirm_committed();
        }
        Ok(SettlementFulfillmentPhaseResult::Advanced(result))
    }
}
