//! 结算履约对账结果的落库收口：deferred、failed 与人工对账。

use chrono::Duration;
use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::ports::settlement::{
    SettlementFulfillmentClaim, SettlementFulfillmentReconcileResult,
    SettlementFulfillmentReconcileStatus,
};
use crate::services::settlement::SettlementFulfillmentService;

impl SettlementFulfillmentService {
    pub(super) async fn defer_now(
        &self,
        claim: SettlementFulfillmentClaim,
        delivery_id: Uuid,
        retry_delay: Duration,
        failure_code: &str,
        safe_detail: &str,
    ) -> Result<SettlementFulfillmentReconcileResult, ApplicationError> {
        let result = SettlementFulfillmentReconcileResult {
            obligation_id: claim.obligation_id,
            delivery_id,
            status: SettlementFulfillmentReconcileStatus::Deferred,
        };
        self.store
            .fail(claim, retry_delay, failure_code.to_owned(), safe_detail.to_owned())
            .await?;
        Ok(result)
    }

    pub(super) async fn failed_now(
        &self,
        claim: SettlementFulfillmentClaim,
        delivery_id: Uuid,
        failure_code: &str,
    ) -> Result<SettlementFulfillmentReconcileResult, ApplicationError> {
        let result = SettlementFulfillmentReconcileResult {
            obligation_id: claim.obligation_id,
            delivery_id,
            status: SettlementFulfillmentReconcileStatus::Failed,
        };
        self.store.mark_failed(claim, failure_code.to_owned()).await?;
        Ok(result)
    }

    pub(super) async fn manual_reconcile_now(
        &self,
        claim: SettlementFulfillmentClaim,
        delivery_id: Uuid,
        failure_code: &str,
    ) -> Result<SettlementFulfillmentReconcileResult, ApplicationError> {
        let result = SettlementFulfillmentReconcileResult {
            obligation_id: claim.obligation_id,
            delivery_id,
            status: SettlementFulfillmentReconcileStatus::ManualReconcile,
        };
        self.store.manual_reconcile(claim, failure_code.to_owned()).await?;
        Ok(result)
    }
}
