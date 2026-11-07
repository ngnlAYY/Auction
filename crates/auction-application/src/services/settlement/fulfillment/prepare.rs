//! 结算履约 `pending` 阶段：把义务制作为 prepared delivery outbox。

use chrono::{DateTime, Duration, Utc};

use super::authorization::authorize_pending_or_prepared;
use super::intent::{build_canonical_delivery, stable_delivery_id, stable_prepare_command_id};
use super::validation::{repository_error, validate_claim};
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, diagnostic_id, record_error,
};
use crate::ports::settlement::{
    SettlementDeliveryPreparation, SettlementFulfillmentClaim, SettlementFulfillmentPhaseResult,
    SettlementFulfillmentResult, SettlementFulfillmentState,
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
    pub(super) async fn prepare_external(
        &self,
        claim: SettlementFulfillmentClaim,
        prepared_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> Result<SettlementFulfillmentPhaseResult<SettlementFulfillmentResult>, ApplicationError>
    {
        validate_claim(&claim)?;
        if claim.state != SettlementFulfillmentState::Pending {
            return Err(repository_error("当前履约阶段不允许生成 prepared delivery"));
        }
        let classified_recipient = claim.recipient.clone();
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
        if claim.recipient != classified_recipient {
            // 身份分支之后发生改名不能把已证明非bot的旧目标换成新目标直接派发。
            let obligation_id = claim.obligation_id;
            self.store
                .fail(
                    claim,
                    retry_delay,
                    "recipient_unavailable".to_owned(),
                    "身份分支后的收件人已变化".to_owned(),
                )
                .await?;
            return Ok(SettlementFulfillmentPhaseResult::Deferred {
                obligation_id,
                failure_code: "recipient_unavailable",
            });
        }
        let canonical = build_canonical_delivery(&intent)?;
        let preparation = SettlementDeliveryPreparation {
            delivery_id: stable_delivery_id(claim.obligation_id),
            prepare_command_id: stable_prepare_command_id(claim.obligation_id),
            request_hash: canonical.request_hash,
            recipient: claim.recipient.clone(),
            payload: canonical.payload,
        };
        let guard = custody.as_mut().map(|prepared| prepared.begin_commit()).transpose().map_err(
            |error| {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "fulfillment",
                    "acquire_prepare_guard",
                    DiagnosticContext {
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        ..Default::default()
                    },
                );
                ApplicationError::FulfillmentDependencyUnavailable
            },
        )?;
        let result = self
            .store
            .prepare_delivery(
                claim,
                preparation,
                prepared_at,
                guard.as_ref().map(|guard| (guard.snapshot(), guard.deadline())),
            )
            .await?;
        if let Some(guard) = guard {
            guard.confirm_committed();
        }
        Ok(SettlementFulfillmentPhaseResult::Advanced(result))
    }
}
