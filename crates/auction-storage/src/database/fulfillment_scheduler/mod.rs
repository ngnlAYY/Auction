//! PostgreSQL 结算履约领取与 prepared delivery outbox 适配器；只持久化 claim/fence 与 owner 间
//! canonical payload。它不读 secret、不发外部 HTTP，delivery 消费须在本地提交后由观察流程回写。

use auction_application::{
    ApplicationError, SettlementDeliveryPreparation, SettlementFulfillmentClaim,
    SettlementFulfillmentProof, SettlementFulfillmentReconcileResult, SettlementFulfillmentResult,
    SettlementFulfillmentStore,
};
use auction_domain::CommandId;
use chrono::{DateTime, Utc};

#[cfg(feature = "postgres")]
use auction_application::{
    SettlementFulfillmentDeliverySnapshot, SettlementFulfillmentOperation,
    SettlementFulfillmentReconcileStatus, SettlementFulfillmentState,
};
#[cfg(feature = "postgres")]
use auction_domain::{
    AuctionId, EquipmentId, LotId, SettlementId, SettlementObligationKind, SettlementWorld,
    SubmissionId,
};

use super::DatabaseStore;
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
mod claims;
#[cfg(feature = "postgres")]
mod delivery;
mod dependencies;
mod internal;
#[cfg(feature = "postgres")]
pub(crate) use internal::assetless_mail_notification;
mod mapping;
#[cfg(feature = "postgres")]
mod reconcile;
#[cfg(feature = "postgres")]
mod sent_lookup;

#[cfg(feature = "postgres")]
use super::delivery_audit::{ACTIVATE, DeliveryAudit, PREPARE, append as append_delivery_audit};
#[cfg(feature = "postgres")]
use claims::*;
#[cfg(feature = "postgres")]
use delivery::*;
use mapping::*;
#[cfg(feature = "postgres")]
use reconcile::*;

#[cfg(feature = "postgres")]
use sqlx::{Postgres, Row, postgres::PgRow};

impl SettlementFulfillmentStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> auction_application::ApplicationFuture<'_, Vec<SettlementFulfillmentClaim>> {
        Box::pin(async move {
            if worker_id.is_empty()
                || worker_id != worker_id.trim()
                || limit == 0
                || lease_until <= now
            {
                return Err(repository_error("结算履约领取参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    claim_postgres(store.pool(), &worker_id, now, lease_until, limit)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }

    fn prepare_delivery<'a>(
        &'a self,
        claim: SettlementFulfillmentClaim,
        preparation: SettlementDeliveryPreparation,
        _prepared_at: DateTime<Utc>,
        _custody: Option<(
            &'a auction_application::SettlementInternalEvidenceSnapshot,
            std::time::Instant,
        )>,
    ) -> auction_application::ApplicationFuture<'a, SettlementFulfillmentResult> {
        Box::pin(async move {
            if preparation.recipient != claim.recipient || preparation.payload.is_null() {
                return Err(repository_error("结算履约 prepared payload 与 claim 不一致"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    prepare_postgres(store.pool(), &claim, &preparation, _prepared_at, _custody)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }

    fn activate_delivery<'a>(
        &'a self,
        _claim: SettlementFulfillmentClaim,
        _activate_command_id: CommandId,
        _activate_request_hash: auction_application::RequestHash,
        _activated_at: DateTime<Utc>,
        _next_attempt_at: Option<DateTime<Utc>>,
        _custody: Option<(
            &'a auction_application::SettlementInternalEvidenceSnapshot,
            std::time::Instant,
        )>,
    ) -> auction_application::ApplicationFuture<
        'a,
        auction_application::SettlementFulfillmentActivationResult,
    > {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => activate_postgres(
                    store.pool(),
                    &_claim,
                    _activate_command_id,
                    _activate_request_hash,
                    _activated_at,
                    _next_attempt_at,
                    _custody,
                )
                .await
                .map_err(ApplicationError::from),
            }
        })
    }

    fn satisfy(
        &self,
        _claim: SettlementFulfillmentClaim,
        _proof: SettlementFulfillmentProof,
    ) -> auction_application::ApplicationFuture<'_, SettlementFulfillmentReconcileResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => satisfy_postgres(store.pool(), &_claim, &_proof)
                    .await
                    .map_err(ApplicationError::from),
            }
        })
    }

    fn manual_reconcile(
        &self,
        _claim: SettlementFulfillmentClaim,
        failure_code: String,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(failure_code, 64);
            if !valid_failure_code(&failure_code) {
                return Err(repository_error("结算履约人工复核代码无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    manual_reconcile_postgres(store.pool(), &_claim, &failure_code)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }

    fn mark_failed(
        &self,
        _claim: SettlementFulfillmentClaim,
        failure_code: String,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(failure_code, 64);
            if !valid_failure_code(&failure_code) {
                return Err(repository_error("结算履约确定失败代码无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => mark_failed_postgres(store.pool(), &_claim, &failure_code)
                    .await
                    .map_err(ApplicationError::from),
            }
        })
    }

    fn fail(
        &self,
        _claim: SettlementFulfillmentClaim,
        _retry_delay: chrono::Duration,
        failure_code: String,
        safe_detail: String,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(failure_code, 64);
            let safe_detail = bounded_text(safe_detail, 512);
            if !valid_failure_code(&failure_code) || safe_detail.is_empty() {
                return Err(repository_error("结算履约失败记录无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    fail_postgres(store.pool(), &_claim, _retry_delay, &failure_code, &safe_detail)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }
}
