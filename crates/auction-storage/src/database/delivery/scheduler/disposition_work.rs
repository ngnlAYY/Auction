//! 寄售处置工作的持久领取、重试、核对与收口。

use auction_application::consignment::{ConsignmentError, ConsignmentFuture, DispositionAction};
use auction_application::core::lower_hex;
use auction_application::mail::{
    DispositionObservationEvidence, DispositionReconcileReason, DispositionRetryReason,
    DispositionWorkClaim, DispositionWorkStore, SignedConsignmentProof,
};
use auction_time::Timestamp;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::repository::DepositRepository;
use crate::database::mail::originals::invariant;
use crate::database::mail::originals::work_claim::{
    claim_lease_micros, claim_selected_in_tx, disposition_claim_columns, disposition_claimable,
};

impl DispositionWorkStore for DepositRepository {
    fn claim_due(
        &self,
        worker_id: String,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> ConsignmentFuture<'_, Option<DispositionWorkClaim>> {
        Box::pin(async move {
            claim_due(self, &worker_id, now, lease_until)
                .await
                .map_err(WriteError::into_application)
        })
    }

    fn check_current(&self, claim: &DispositionWorkClaim) -> ConsignmentFuture<'_, ()> {
        let claim = claim.clone();
        Box::pin(
            async move { check_current(self, &claim).await.map_err(WriteError::into_application) },
        )
    }

    fn release_for_retry(
        &self,
        claim: &DispositionWorkClaim,
        reason: DispositionRetryReason,
        retry_at: Timestamp,
    ) -> ConsignmentFuture<'_, ()> {
        let claim = claim.clone();
        Box::pin(async move {
            release_for_retry(self, &claim, reason, retry_at)
                .await
                .map_err(WriteError::into_application)
        })
    }

    fn record_observation(
        &self,
        claim: &DispositionWorkClaim,
        evidence: DispositionObservationEvidence,
    ) -> ConsignmentFuture<'_, ()> {
        let claim = claim.clone();
        Box::pin(async move {
            record_observation(self, &claim, evidence).await.map_err(WriteError::into_application)
        })
    }

    fn reconcile(
        &self,
        claim: &DispositionWorkClaim,
        reason: DispositionReconcileReason,
        evidence: Option<DispositionObservationEvidence>,
    ) -> ConsignmentFuture<'_, ()> {
        let claim = claim.clone();
        Box::pin(async move {
            reconcile(self, &claim, reason, evidence).await.map_err(WriteError::into_application)
        })
    }

    fn complete(
        &self,
        claim: &DispositionWorkClaim,
        proof: &SignedConsignmentProof,
    ) -> ConsignmentFuture<'_, ()> {
        let claim = claim.clone();
        let proof = proof.clone();
        Box::pin(async move {
            complete(self, &claim, &proof).await.map_err(WriteError::into_application)
        })
    }
}

async fn claim_due(
    repository: &DepositRepository,
    worker_id: &str,
    now: Timestamp,
    lease_until: Timestamp,
) -> Result<Option<DispositionWorkClaim>, WriteError> {
    let lease_micros = claim_lease_micros(worker_id, now, lease_until)?;
    let mut tx = repository.store().pool().begin().await?;
    // 退件接管（`consignment_return_intakes`）建立的 Take 工作只由退件起写链领取与起写：它的起写事实
    // 与现金阻断、消费都绑定 intake，期 4 的处置执行不得领走（不变量 4：同一资产只有一条外写路径）。
    let row = sqlx::query(concat!(
        "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         SELECT ",
        disposition_claim_columns!(),
        "
         FROM public.mail_disposition_work AS w
         JOIN public.deliveries AS d ON d.id = w.delivery_id
         WHERE w.action IN ('take', 'return')
           AND NOT EXISTS (SELECT 1 FROM public.consignment_return_intakes AS intake
                           WHERE intake.take_work_id = w.id)
           AND ",
        disposition_claimable!(),
        "
         ORDER BY d.created_at, w.id
         LIMIT 1
         FOR UPDATE OF d SKIP LOCKED",
    ))
    .bind(lease_micros)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let claim = claim_selected_in_tx(&mut tx, &row).await?;
    tx.commit().await?;
    Ok(Some(claim))
}

async fn check_current(
    repository: &DepositRepository,
    claim: &DispositionWorkClaim,
) -> Result<(), WriteError> {
    let current: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM public.deliveries
            WHERE id = $1 AND status = 'claimed' AND lease_token = $2
              AND fencing_version = $3 AND lease_expires_at > clock_timestamp()
         )",
    )
    .bind(claim.delivery_id)
    .bind(claim.claim_token)
    .bind(i64::try_from(claim.fence)?)
    .fetch_one(repository.store().pool())
    .await?;
    if !current {
        return Err(WriteError::Application(ConsignmentError::LeaseLost));
    }
    Ok(())
}

async fn release_for_retry(
    repository: &DepositRepository,
    claim: &DispositionWorkClaim,
    _reason: DispositionRetryReason,
    retry_at: Timestamp,
) -> Result<(), WriteError> {
    // 与通用释放一致，退回 prepared 时清掉领取事实；下一次领取会重新写入，
    // 否则该来源的 prepared 行会因残留 claimed_at 被 Admin 读模型判为损坏（不变量 4、6）。
    let updated = sqlx::query(
        "UPDATE public.deliveries
         SET status = 'prepared', lease_token = NULL, lease_expires_at = NULL,
             claimed_at = NULL,
             updated_at = clock_timestamp(),
             payload = payload || jsonb_build_object('retry_at', to_char($4 AT TIME ZONE 'UTC',
                 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'))
         WHERE id = $1 AND status = 'claimed' AND lease_token = $2 AND fencing_version = $3",
    )
    .bind(claim.delivery_id)
    .bind(claim.claim_token)
    .bind(i64::try_from(claim.fence)?)
    .bind(retry_at)
    .execute(repository.store().pool())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(WriteError::Application(ConsignmentError::LeaseLost));
    }
    Ok(())
}

async fn record_observation(
    repository: &DepositRepository,
    claim: &DispositionWorkClaim,
    evidence: DispositionObservationEvidence,
) -> Result<(), WriteError> {
    check_current(repository, claim).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.mail_envelope_observations WHERE id = $1)",
    )
    .bind(evidence.observation_id)
    .fetch_one(repository.store().pool())
    .await?;
    if !exists {
        return Err(invariant("consignment_work_observation_missing"));
    }
    Ok(())
}

async fn reconcile(
    repository: &DepositRepository,
    claim: &DispositionWorkClaim,
    reason: DispositionReconcileReason,
    evidence: Option<DispositionObservationEvidence>,
) -> Result<(), WriteError> {
    if let Some(evidence) = evidence {
        record_observation(repository, claim, evidence).await?;
    } else {
        check_current(repository, claim).await?;
    }
    let updated = sqlx::query(
        "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         UPDATE public.deliveries
         SET status = 'manual_reconcile', lease_token = NULL, lease_expires_at = NULL,
             failure_code = $4, completed_at = (SELECT at FROM sql_clock),
             updated_at = (SELECT at FROM sql_clock)
         WHERE id = $1 AND status = 'claimed' AND lease_token = $2 AND fencing_version = $3",
    )
    .bind(claim.delivery_id)
    .bind(claim.claim_token)
    .bind(i64::try_from(claim.fence)?)
    .bind(reason.as_str())
    .execute(repository.store().pool())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(WriteError::Application(ConsignmentError::LeaseLost));
    }
    Ok(())
}

async fn complete(
    repository: &DepositRepository,
    claim: &DispositionWorkClaim,
    proof: &SignedConsignmentProof,
) -> Result<(), WriteError> {
    check_current(repository, claim).await?;
    let conclusion = match proof.action() {
        DispositionAction::Take => "take_proven",
        DispositionAction::Return => "return_proven",
    };
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM public.mail_disposition_proofs
            WHERE work_id = $1 AND delivery_id = $2 AND fence = $3 AND observation_id = $4
              AND digest = $5 AND conclusion = $6
         )",
    )
    .bind(claim.work_id)
    .bind(claim.delivery_id)
    .bind(i64::try_from(claim.fence)?)
    .bind(proof.observation_id())
    .bind(proof.proof_digest().as_slice())
    .bind(conclusion)
    .fetch_one(repository.store().pool())
    .await?;
    if !valid {
        return Err(invariant("consignment_work_proof_missing"));
    }
    let updated = sqlx::query(
        "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         UPDATE public.deliveries
         SET status = 'succeeded', lease_token = NULL, lease_expires_at = NULL,
             external_reference = $4, completed_at = (SELECT at FROM sql_clock),
             updated_at = (SELECT at FROM sql_clock)
         WHERE id = $1 AND status = 'claimed' AND lease_token = $2 AND fencing_version = $3",
    )
    .bind(claim.delivery_id)
    .bind(claim.claim_token)
    .bind(i64::try_from(claim.fence)?)
    .bind(lower_hex(proof.proof_digest().as_slice()))
    .execute(repository.store().pool())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(WriteError::Application(ConsignmentError::LeaseLost));
    }
    Ok(())
}
