//! 寄售处置工作的持久领取、重试、核对与收口。

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DepositBusinessRejection, DispositionAction,
};
use auction_application::core::lower_hex;
use auction_application::mail::{
    DispositionObservationEvidence, DispositionReconcileReason, DispositionRetryReason,
    DispositionWorkClaim, DispositionWorkStore, SignedConsignmentProof,
};
use auction_time::Timestamp;
use sqlx::Row;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::repository::DepositRepository;
use crate::database::mail::originals::disposition::manifest_from_payload;
use crate::database::mail::originals::identity::parse_world;
use crate::database::mail::originals::{invariant, invariant_decoding};

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
    let lease_duration = lease_until.signed_duration_since(now);
    let lease_micros = lease_duration
        .num_microseconds()
        .filter(|value| *value > 0)
        .ok_or_else(|| invariant("consignment_work_claim_input"))?;
    if worker_id.is_empty() {
        return Err(invariant("consignment_work_claim_input"));
    }
    let mut tx = repository.store().pool().begin().await?;
    let row = sqlx::query(
        "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         SELECT w.id, w.decision_id, w.delivery_id, w.action, w.expected_observation_id,
                w.manifest_digest, d.payload, d.fencing_version, d.attempt_count,
                (SELECT at FROM sql_clock) AS claimed_at,
                (SELECT at FROM sql_clock)
                    + ($1 * interval '1 microsecond') AS lease_until
         FROM public.mail_disposition_work AS w
         JOIN public.deliveries AS d ON d.id = w.delivery_id
         WHERE w.action IN ('take', 'return')
           AND d.origin_kind = 'consignment_disposition'
           AND (
               (d.status = 'prepared'
                AND (d.lease_expires_at IS NULL
                     OR d.lease_expires_at <= (SELECT at FROM sql_clock)))
               OR (d.status = 'claimed'
                   AND d.lease_expires_at <= (SELECT at FROM sql_clock))
           )
           AND COALESCE(
                 CASE WHEN pg_input_is_valid(d.payload->>'retry_at', 'timestamptz')
                      THEN (d.payload->>'retry_at')::timestamptz END,
                 '-infinity'::timestamptz) <= (SELECT at FROM sql_clock)
         ORDER BY d.created_at, w.id
         LIMIT 1
         FOR UPDATE OF d SKIP LOCKED",
    )
    .bind(lease_micros)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let work_id: Uuid = row.try_get("id")?;
    let decision_id: Uuid = row.try_get("decision_id")?;
    let delivery_id: Uuid = row.try_get("delivery_id")?;
    let action = match row.try_get::<String, _>("action")?.as_str() {
        "take" => DispositionAction::Take,
        "return" => DispositionAction::Return,
        _ => return Err(invariant("consignment_work_action")),
    };
    let action_code = action.as_str();
    let manifest_digest: &[u8] = row.try_get("manifest_digest")?;
    let manifest_digest = <[u8; 32]>::try_from(manifest_digest)
        .map_err(invariant_decoding("consignment_work_manifest_digest"))?;
    let payload: serde_json::Value = row.try_get("payload")?;
    let manifest = manifest_from_payload(&payload, manifest_digest)?;
    let claim_token = Uuid::new_v4();
    let fencing: i64 = row.try_get("fencing_version")?;
    let attempt_count: i32 = row.try_get("attempt_count")?;
    let claimed_at: Timestamp = row.try_get("claimed_at")?;
    let lease_until: Timestamp = row.try_get("lease_until")?;
    let next_fence =
        fencing.checked_add(1).ok_or_else(|| invariant("consignment_work_fence_overflow"))?;
    let next_attempt = attempt_count
        .checked_add(1)
        .ok_or_else(|| invariant("consignment_work_attempt_overflow"))?;
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'claimed', lease_token = $2, lease_expires_at = $3,
             fencing_version = $4, attempt_count = $5, claimed_at = $6, updated_at = $6,
             payload = payload - 'retry_at'
         WHERE id = $1
           AND (status = 'prepared'
                OR (status = 'claimed' AND lease_expires_at <= $6))",
    )
    .bind(delivery_id)
    .bind(claim_token)
    .bind(lease_until)
    .bind(next_fence)
    .bind(next_attempt)
    .bind(claimed_at)
    .execute(&mut *tx)
    .await?;
    let world = parse_world(
        &sqlx::query_scalar::<_, String>(
            "SELECT world_id FROM public.mail_disposition_work WHERE id = $1",
        )
        .bind(work_id)
        .fetch_one(&mut *tx)
        .await?,
    )?;
    let session = sqlx::query(
        "SELECT r.session_id, s.credential_id, s.credential_version, s.runtime_generation
         FROM public.consignment_mail_readiness AS r
         JOIN public.mail_evidence_sessions AS s ON s.id = r.session_id
         WHERE r.world_id = $1 AND r.valid_until > $2",
    )
    .bind(auction_domain::consignment::settlement_world_code(world))
    .bind(claimed_at)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(session) = session else {
        tx.rollback().await?;
        return Err(WriteError::Application(ConsignmentError::Deposit(
            DepositBusinessRejection::MailUnavailable,
        )));
    };
    let attempt_id = Uuid::new_v4();
    let session_id: Uuid = session.try_get("session_id")?;
    sqlx::query(
        "INSERT INTO public.mail_disposition_attempts
            (id, work_id, delivery_id, fence, credential_id, credential_version,
             runtime_generation, session_id, expected_digest, prepared_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(attempt_id)
    .bind(work_id)
    .bind(delivery_id)
    .bind(next_fence)
    .bind(session.try_get::<Uuid, _>("credential_id")?)
    .bind(session.try_get::<i64, _>("credential_version")?)
    .bind(session.try_get::<i64, _>("runtime_generation")?)
    .bind(session_id)
    .bind(manifest_digest.as_slice())
    .bind(claimed_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let _ = action_code;
    Ok(Some(DispositionWorkClaim {
        work_id,
        final_decision_id: decision_id,
        delivery_id,
        action,
        attempt_id,
        fence: u64::try_from(next_fence).map_err(invariant_decoding("consignment_work_fence"))?,
        claim_token,
        attempt_count: u32::try_from(next_attempt)
            .map_err(invariant_decoding("consignment_work_attempt"))?,
        lease_until,
        manifest,
    }))
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
