//! PostgreSQL 结算履约领取与 prepared delivery outbox 适配器；只持久化 claim/fence 与 owner 间
//! canonical payload。它不读 secret、不发外部 HTTP，delivery 消费须在本地提交后由观察流程回写。

use auction_application::core::ApplicationError;
use auction_application::settlement::{
    SettlementDeliveryPreparation, SettlementFulfillmentClaim, SettlementFulfillmentProof,
    SettlementFulfillmentReconcileResult, SettlementFulfillmentResult, SettlementFulfillmentStore,
};
use auction_domain::CommandId;
use chrono::{DateTime, Utc};

use crate::database::DatabaseStore;

mod claims;
mod delivery;
mod dependencies;
mod internal;
pub(crate) use internal::{assetless_mail_notification, exclude_equipment_loss_conflicts};
mod mapping;
mod reconcile;
mod sent_lookup;

use claims::*;
use delivery::*;
use mapping::*;
use reconcile::*;

impl SettlementFulfillmentStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> auction_application::auction::ApplicationFuture<'_, Vec<SettlementFulfillmentClaim>> {
        Box::pin(async move {
            if worker_id.is_empty()
                || worker_id != worker_id.trim()
                || limit == 0
                || lease_until <= now
            {
                return Err(repository_error("结算履约领取参数无效"));
            }
            claim_postgres(self.pool(), &worker_id, now, lease_until, limit)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn prepare_delivery<'a>(
        &'a self,
        claim: SettlementFulfillmentClaim,
        preparation: SettlementDeliveryPreparation,
        prepared_at: DateTime<Utc>,
        custody: Option<(
            &'a auction_application::settlement::SettlementInternalEvidenceSnapshot,
            std::time::Instant,
        )>,
    ) -> auction_application::auction::ApplicationFuture<'a, SettlementFulfillmentResult> {
        Box::pin(async move {
            if preparation.recipient != claim.recipient || preparation.payload.is_null() {
                return Err(repository_error("结算履约 prepared payload 与 claim 不一致"));
            }
            prepare_postgres(
                self.pool(),
                self.notification_metrics(),
                &claim,
                &preparation,
                prepared_at,
                custody,
            )
            .await
            .map_err(ApplicationError::from)
        })
    }

    fn activate_delivery<'a>(
        &'a self,
        claim: SettlementFulfillmentClaim,
        activate_command_id: CommandId,
        activate_request_hash: auction_application::core::RequestHash,
        activated_at: DateTime<Utc>,
        next_attempt_at: Option<DateTime<Utc>>,
        custody: Option<(
            &'a auction_application::settlement::SettlementInternalEvidenceSnapshot,
            std::time::Instant,
        )>,
    ) -> auction_application::auction::ApplicationFuture<
        'a,
        auction_application::settlement::SettlementFulfillmentActivationResult,
    > {
        Box::pin(async move {
            activate_postgres(
                self.pool(),
                &claim,
                activate_command_id,
                activate_request_hash,
                activated_at,
                next_attempt_at,
                custody,
            )
            .await
            .map_err(ApplicationError::from)
        })
    }

    fn satisfy(
        &self,
        claim: SettlementFulfillmentClaim,
        proof: SettlementFulfillmentProof,
    ) -> auction_application::auction::ApplicationFuture<'_, SettlementFulfillmentReconcileResult>
    {
        Box::pin(async move {
            satisfy_postgres(self.pool(), &claim, &proof).await.map_err(ApplicationError::from)
        })
    }

    fn manual_reconcile(
        &self,
        claim: SettlementFulfillmentClaim,
        failure_code: String,
    ) -> auction_application::auction::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(failure_code, 64);
            if !valid_failure_code(&failure_code) {
                return Err(repository_error("结算履约人工复核代码无效"));
            }
            manual_reconcile_postgres(self.pool(), &claim, &failure_code)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn mark_failed(
        &self,
        claim: SettlementFulfillmentClaim,
        failure_code: String,
    ) -> auction_application::auction::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(failure_code, 64);
            if !valid_failure_code(&failure_code) {
                return Err(repository_error("结算履约确定失败代码无效"));
            }
            mark_failed_postgres(self.pool(), &claim, &failure_code)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn fail(
        &self,
        claim: SettlementFulfillmentClaim,
        retry_delay: chrono::Duration,
        failure_code: String,
        safe_detail: String,
    ) -> auction_application::auction::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(failure_code, 64);
            let safe_detail = bounded_text(safe_detail, 512);
            if !valid_failure_code(&failure_code) || safe_detail.is_empty() {
                return Err(repository_error("结算履约失败记录无效"));
            }
            fail_postgres(self.pool(), &claim, retry_delay, &failure_code, &safe_detail)
                .await
                .map_err(ApplicationError::from)
        })
    }
}
