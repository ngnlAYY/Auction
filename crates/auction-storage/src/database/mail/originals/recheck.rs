//! 隔离重检的 Admin 排队与持久工作领取/核对/重试。

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DepositActor, DepositBusinessRejection, DepositOperation,
    ObservationReceipt, RecheckQuarantineCommand, RecheckReceipt,
};
use auction_application::mail::{
    DispositionReconcileReason, DispositionRetryReason, QuarantineMailLocator,
    QuarantineRecheckClaim, QuarantineRecheckStore,
};
use auction_application::settlement::{
    INTERNAL_EVIDENCE_PROTOCOL, MailAccountEvidenceBinding, MailRuntimeBinding,
};
use auction_domain::consignment::OriginalMailKey;
use auction_time::Timestamp;
use sqlx::Row;
use uuid::Uuid;

use super::identity::{load_quarantine, parse_world};
use super::{Outcome, conflict, invariant, replay_receipt, unavailable_decoding};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{DepositArbitration, DepositTarget};
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::deposit::repository::DepositRepository;

pub(crate) async fn recheck_quarantine_port(
    repository: &DepositRepository,
    command: RecheckQuarantineCommand,
) -> Result<RecheckReceipt, ConsignmentError> {
    let pool = repository.store().pool();
    let mut pre =
        pool.acquire().await.map_err(|error| WriteError::from(error).into_application())?;
    let quarantine = load_quarantine(&mut pre, command.quarantine_id)
        .await
        .map_err(WriteError::into_application)?
        .ok_or(ConsignmentError::Deposit(DepositBusinessRejection::ListingNotActive))?;
    drop(pre);
    let key = quarantine.mail_key.clone();
    let sellers: Vec<Uuid> = Vec::new();
    let equipment: Vec<auction_domain::consignment::ListingEquipmentIdentity> = Vec::new();
    let keys = LockKeys {
        command_id: command.command_id.as_uuid(),
        sellers: &sellers,
        equipment: &equipment,
    };
    let mail_keys = [key.clone()];
    let input = command;
    let transaction =
        with_deposit_transaction(repository.store(), keys, &mail_keys, &input, |input, tx| {
            let sellers = sellers.clone();
            let equipment = equipment.clone();
            let key = key.clone();
            Box::pin(async move {
                tx.ensure_lock_set(&sellers, &equipment, std::slice::from_ref(&key))?;
                let arbitration = tx
                    .arbitrate(
                        DepositActor::Admin(input.admin_actor.clone()),
                        DepositOperation::RecheckQuarantine,
                        DepositTarget::Quarantine(input.quarantine_id),
                        input.request_hash,
                        input.request_id.as_deref(),
                    )
                    .await?;
                let mut writer = match arbitration {
                    DepositArbitration::Replay(stored) => {
                        return match replay_receipt::<RecheckReceipt>(&stored)? {
                            Outcome::Accepted(receipt) => Ok(Outcome::Accepted(receipt)),
                            Outcome::Rejected(_) => {
                                Err(invariant("consignment_recheck_replay_rejected"))
                            }
                        };
                    }
                    DepositArbitration::First(writer) => writer,
                };
                let now = database_now(writer.connection()).await?;
                let row = sqlx::query(
                    "SELECT version, state FROM public.mail_quarantines
                     WHERE id = $1",
                )
                .bind(input.quarantine_id)
                .fetch_optional(writer.connection())
                .await?
                .ok_or_else(|| conflict(DepositBusinessRejection::ListingNotActive))?;
                let version: i64 = row.try_get("version")?;
                if row.try_get::<String, _>("state")? != "active" {
                    return Err(conflict(DepositBusinessRejection::DispositionConflict));
                }
                if u64::try_from(version).ok() != Some(input.expected_version) {
                    return Err(conflict(DepositBusinessRejection::StaleVersion));
                }
                let active: bool = sqlx::query_scalar(
                    "SELECT EXISTS (
                        SELECT 1 FROM public.mail_quarantine_rechecks
                        WHERE quarantine_id = $1 AND state IN ('queued', 'leased')
                     )",
                )
                .bind(input.quarantine_id)
                .fetch_one(writer.connection())
                .await?;
                if active {
                    return Err(conflict(DepositBusinessRejection::DispositionConflict));
                }
                let work_id = Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO public.mail_quarantine_rechecks
                        (id, quarantine_id, requested_version, command_id, state, available_at,
                         lease_token, lease_until, fence, attempt_count, completed_at)
                     VALUES ($1, $2, $3, $4, 'queued', $5, NULL, NULL, 0, 0, NULL)",
                )
                .bind(work_id)
                .bind(input.quarantine_id)
                .bind(version)
                .bind(input.command_id.as_uuid())
                .bind(now)
                .execute(writer.connection())
                .await?;
                let receipt =
                    RecheckReceipt::queued(input.command_id, input.quarantine_id, work_id);
                let _ = writer.finish(receipt.clone(), now).await?;
                Ok(Outcome::Accepted(receipt))
            })
        });
    transaction.await.map_err(WriteError::into_application)?.into_result()
}

impl QuarantineRecheckStore for DepositRepository {
    fn claim_due(
        &self,
        worker_id: String,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> ConsignmentFuture<'_, Option<QuarantineRecheckClaim>> {
        Box::pin(async move { claim_recheck(self, worker_id, now, lease_until).await })
    }

    fn check_current(&self, claim: &QuarantineRecheckClaim) -> ConsignmentFuture<'_, ()> {
        let claim = claim.clone();
        Box::pin(async move { check_recheck(self, &claim).await })
    }

    fn release_for_retry(
        &self,
        claim: &QuarantineRecheckClaim,
        _reason: DispositionRetryReason,
        retry_at: Timestamp,
    ) -> ConsignmentFuture<'_, ()> {
        let claim = claim.clone();
        Box::pin(async move { retry_recheck(self, &claim, retry_at).await })
    }

    fn complete(
        &self,
        claim: &QuarantineRecheckClaim,
        _receipt: &ObservationReceipt,
    ) -> ConsignmentFuture<'_, ()> {
        let claim = claim.clone();
        Box::pin(async move { finish_recheck(self, &claim, "done").await })
    }

    fn reconcile(
        &self,
        claim: &QuarantineRecheckClaim,
        _reason: DispositionReconcileReason,
    ) -> ConsignmentFuture<'_, ()> {
        let claim = claim.clone();
        Box::pin(async move { finish_recheck(self, &claim, "cancelled").await })
    }
}

async fn claim_recheck(
    repository: &DepositRepository,
    worker_id: String,
    now: Timestamp,
    lease_until: Timestamp,
) -> Result<Option<QuarantineRecheckClaim>, ConsignmentError> {
    let lease_duration = lease_until.signed_duration_since(now);
    let lease_micros = lease_duration
        .num_microseconds()
        .filter(|value| *value > 0)
        .ok_or(ConsignmentError::InvalidDepositRequest)?;
    if worker_id.is_empty() {
        return Err(ConsignmentError::InvalidDepositRequest);
    }
    let pool = repository.store().pool();
    let mut tx = pool.begin().await.map_err(storage)?;
    let row = sqlx::query(
        "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         SELECT r.id, r.quarantine_id, r.requested_version, r.attempt_count, r.fence,
                q.bot_uid, q.world_id, q.external_mid, q.reread_locator,
                (SELECT at FROM sql_clock) AS claimed_at,
                (SELECT at FROM sql_clock)
                    + ($1 * interval '1 microsecond') AS lease_until
         FROM public.mail_quarantine_rechecks AS r
         JOIN public.mail_quarantines AS q ON q.id = r.quarantine_id
         WHERE (
             (r.state = 'queued' AND r.available_at <= (SELECT at FROM sql_clock))
             OR (r.state = 'leased' AND r.lease_until <= (SELECT at FROM sql_clock))
         )
         ORDER BY r.available_at, r.id
         LIMIT 1
         FOR UPDATE OF r SKIP LOCKED",
    )
    .bind(lease_micros)
    .fetch_optional(&mut *tx)
    .await
    .map_err(storage)?;
    let Some(row) = row else {
        tx.commit().await.map_err(storage)?;
        return Ok(None);
    };
    let work_id: Uuid = row.try_get("id").map_err(storage)?;
    let quarantine_id: Uuid = row.try_get("quarantine_id").map_err(storage)?;
    let version: i64 = row.try_get("requested_version").map_err(storage)?;
    let attempt_count: i32 = row.try_get("attempt_count").map_err(storage)?;
    let fence: i64 = row.try_get("fence").map_err(storage)?;
    let claimed_at: Timestamp = row.try_get("claimed_at").map_err(storage)?;
    let lease_until: Timestamp = row.try_get("lease_until").map_err(storage)?;
    let claim_token = Uuid::new_v4();
    let next_fence = fence.checked_add(1).ok_or(ConsignmentError::Unavailable)?;
    let next_attempt = attempt_count.checked_add(1).ok_or(ConsignmentError::Unavailable)?;
    let updated = sqlx::query(
        "UPDATE public.mail_quarantine_rechecks
         SET state = 'leased', lease_token = $2, lease_until = $3, fence = $4,
             attempt_count = $5
         WHERE id = $1
           AND ((state = 'queued' AND available_at <= $6)
                OR (state = 'leased' AND lease_until <= $6))",
    )
    .bind(work_id)
    .bind(claim_token)
    .bind(lease_until)
    .bind(next_fence)
    .bind(next_attempt)
    .bind(claimed_at)
    .execute(&mut *tx)
    .await
    .map_err(storage)?;
    if updated.rows_affected() != 1 {
        tx.rollback().await.map_err(storage)?;
        return Ok(None);
    }
    let world = parse_world(&row.try_get::<String, _>("world_id").map_err(storage)?)
        .map_err(WriteError::into_application)?;
    let bot_uid: String = row.try_get("bot_uid").map_err(storage)?;
    let external_mid: String = row.try_get("external_mid").map_err(storage)?;
    let key = OriginalMailKey::new(&bot_uid, world, &external_mid)
        .map_err(unavailable_decoding("consignment_quarantine_mail_key"))?;
    let observation =
        super::observations::load_latest_quarantine_observation(&mut tx, &key, quarantine_id)
            .await
            .map_err(WriteError::into_application)?
            .ok_or(ConsignmentError::Unavailable)?;
    // 不变量 4、6：重检不需要 poll 租约；它只使用自己的持久 lease/fence。这里从最新观察的
    // 证据会话重建同名换 UID 判定所需的绑定，缺失或损坏时失败关闭。
    let binding_row = sqlx::query(
        "SELECT bot_uid, world_id, player_name, credential_id, credential_version,
                runtime_generation, observed_at, protocol_version, binding_digest
         FROM public.mail_evidence_sessions WHERE id = $1",
    )
    .bind(observation.session_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(storage)?
    .ok_or(ConsignmentError::Unavailable)?;
    let binding_digest: Vec<u8> = binding_row.try_get("binding_digest").map_err(storage)?;
    let binding_digest = <[u8; 32]>::try_from(binding_digest.as_slice())
        .map_err(unavailable_decoding("consignment_recheck_binding_digest"))?;
    let protocol_version: String = binding_row.try_get("protocol_version").map_err(storage)?;
    if protocol_version != INTERNAL_EVIDENCE_PROTOCOL {
        return Err(ConsignmentError::Unavailable);
    }
    let binding = MailAccountEvidenceBinding {
        credential_id: binding_row.try_get("credential_id").map_err(storage)?,
        credential_version: binding_row.try_get("credential_version").map_err(storage)?,
        runtime: MailRuntimeBinding {
            generation: binding_row.try_get("runtime_generation").map_err(storage)?,
        },
        authenticated_uid: binding_row.try_get("bot_uid").map_err(storage)?,
        player_name: binding_row.try_get("player_name").map_err(storage)?,
        world: parse_world(&binding_row.try_get::<String, _>("world_id").map_err(storage)?)
            .map_err(WriteError::into_application)?,
        observed_at: binding_row.try_get("observed_at").map_err(storage)?,
        protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
        digest: binding_digest,
    };
    // 只用会话行重建同名换 UID 判定所需的身份；摘要仍按存储值保留。测试夹具与会话写入
    // 路径不必重算规范摘要，但缺字段、错世界或错 Bot 一律失败关闭。
    if binding.credential_id.is_nil()
        || binding.credential_version <= 0
        || binding.runtime.generation <= 0
        || binding.authenticated_uid != bot_uid
        || binding.player_name.is_empty()
        || binding.world != world
        || binding.digest == [0; 32]
    {
        return Err(ConsignmentError::Unavailable);
    }
    tx.commit().await.map_err(storage)?;
    Ok(Some(QuarantineRecheckClaim {
        work_id,
        quarantine_id,
        quarantine_version: u64::try_from(version)
            .map_err(unavailable_decoding("consignment_quarantine_version"))?,
        mail_key: key,
        locator: QuarantineMailLocator {
            world: observation.world,
            external_mail_id: external_mid,
            sender: observation.sender.clone().unwrap_or_default(),
            subject: observation
                .legacy_return_evidence
                .subject
                .clone()
                .or_else(|| observation.subject_is_consign.then(|| "consign".to_owned()))
                .unwrap_or_default(),
            received_minute_raw: observation.received_minute_raw,
            received_at: observation.received_at,
        },
        previous_observation_id: observation.observation_id,
        previous_binding: Some(binding),
        claim_token,
        attempt_count: u32::try_from(next_attempt)
            .map_err(unavailable_decoding("consignment_recheck_attempt"))?,
        fence: u64::try_from(next_fence)
            .map_err(unavailable_decoding("consignment_recheck_fence"))?,
        lease_until,
    }))
}

async fn check_recheck(
    repository: &DepositRepository,
    claim: &QuarantineRecheckClaim,
) -> Result<(), ConsignmentError> {
    let current: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM public.mail_quarantine_rechecks
            WHERE id = $1 AND quarantine_id = $2 AND state = 'leased'
              AND lease_token = $3 AND fence = $4 AND lease_until > clock_timestamp()
         )",
    )
    .bind(claim.work_id)
    .bind(claim.quarantine_id)
    .bind(claim.claim_token)
    .bind(i64::try_from(claim.fence).map_err(unavailable_decoding("consignment_claim_fence"))?)
    .fetch_one(repository.store().pool())
    .await
    .map_err(storage)?;
    if !current {
        return Err(ConsignmentError::LeaseLost);
    }
    Ok(())
}

async fn retry_recheck(
    repository: &DepositRepository,
    claim: &QuarantineRecheckClaim,
    retry_at: Timestamp,
) -> Result<(), ConsignmentError> {
    check_recheck(repository, claim).await?;
    let updated = sqlx::query(
        "UPDATE public.mail_quarantine_rechecks
         SET state = 'queued', lease_token = NULL, lease_until = NULL, available_at = $5
         WHERE id = $1 AND quarantine_id = $2 AND state = 'leased'
           AND lease_token = $3 AND fence = $4",
    )
    .bind(claim.work_id)
    .bind(claim.quarantine_id)
    .bind(claim.claim_token)
    .bind(i64::try_from(claim.fence).map_err(unavailable_decoding("consignment_claim_fence"))?)
    .bind(retry_at)
    .execute(repository.store().pool())
    .await
    .map_err(storage)?;
    if updated.rows_affected() != 1 {
        return Err(ConsignmentError::LeaseLost);
    }
    Ok(())
}

async fn finish_recheck(
    repository: &DepositRepository,
    claim: &QuarantineRecheckClaim,
    state: &'static str,
) -> Result<(), ConsignmentError> {
    check_recheck(repository, claim).await?;
    let updated = sqlx::query(
        "UPDATE public.mail_quarantine_rechecks
         SET state = $5, lease_token = NULL, lease_until = NULL,
             completed_at = clock_timestamp()
         WHERE id = $1 AND quarantine_id = $2 AND state = 'leased'
           AND lease_token = $3 AND fence = $4",
    )
    .bind(claim.work_id)
    .bind(claim.quarantine_id)
    .bind(claim.claim_token)
    .bind(i64::try_from(claim.fence).map_err(unavailable_decoding("consignment_claim_fence"))?)
    .bind(state)
    .execute(repository.store().pool())
    .await
    .map_err(storage)?;
    if updated.rows_affected() != 1 {
        return Err(ConsignmentError::LeaseLost);
    }
    Ok(())
}

fn storage(error: sqlx::Error) -> ConsignmentError {
    WriteError::from(error).into_application()
}
