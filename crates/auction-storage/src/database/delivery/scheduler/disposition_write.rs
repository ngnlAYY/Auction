//! 寄售资产处置的起写许可、证明应用与工作领取。
//!
//! 首次成功起写只由 `mail_disposition_write_starts.work_id` 唯一行产生；证明必须引用原
//! attempt/fence。领取、释放、核对与收口都以 delivery 的 token/fence 条件更新。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, DepositActor, DepositBusinessRejection, DepositOperation, DispositionAction,
    DispositionStore, MailProofKind, ProofDisposition, ProofReceipt, StartReceipt,
    WriteStartOutcome,
};
use auction_domain::CommandId;
use auction_domain::consignment::OriginalMailKey;
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{DepositArbitration, DepositTarget};
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::deposit::repository::DepositRepository;
use crate::database::mail::originals::identity::{MailKeyColumns, parse_world};
use crate::database::mail::originals::proofs::{
    ProofWrite, existing_proof_receipt, insert_start, load_attempt, load_proof_identity,
    load_start, load_work_for_update, upsert_proof,
};
use crate::database::mail::originals::{
    Outcome, conflict, invariant, invariant_decoding, replay_receipt,
};

impl DispositionStore for DepositRepository {
    fn start_disposition_write(
        &self,
        command: auction_application::consignment::StartDispositionWriteCommand,
    ) -> auction_application::consignment::ConsignmentFuture<'_, WriteStartOutcome> {
        Box::pin(async move { start_port(self, command).await })
    }

    fn apply_mail_proof(
        &self,
        command: auction_application::consignment::ApplyMailProofCommand,
    ) -> auction_application::consignment::ConsignmentFuture<'_, ProofReceipt> {
        Box::pin(async move { apply_port(self, command).await })
    }
}

async fn start_port(
    repository: &DepositRepository,
    command: auction_application::consignment::StartDispositionWriteCommand,
) -> Result<WriteStartOutcome, ConsignmentError> {
    let (key, sellers, equipment) = pre_read_work(repository, command.work_id).await?;
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
                        DepositActor::Worker,
                        DepositOperation::StartDispositionWrite,
                        DepositTarget::FinalDecision(input.final_decision_id),
                        input.request_hash,
                        None,
                    )
                    .await?;
                let mut writer = match arbitration {
                    DepositArbitration::Replay(stored) => {
                        return match replay_receipt::<StartReceipt>(&stored)? {
                            Outcome::Accepted(receipt) => {
                                Ok(StartOutcome::AlreadyCommitted(receipt))
                            }
                            Outcome::Rejected(_) => {
                                Err(invariant("consignment_start_replay_rejected"))
                            }
                        };
                    }
                    DepositArbitration::First(writer) => writer,
                };
                let work = load_work_for_update(writer.connection(), input.work_id)
                    .await?
                    .ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
                if work.decision_id != input.final_decision_id
                    || work.delivery_id != input.delivery_id
                    || work.action != input.action
                    || work.manifest_digest != input.expected_manifest_digest
                {
                    return Err(conflict(DepositBusinessRejection::ActionNotAllowed));
                }
                // 不变量 1、3、6：每个 K 恰有一个最终决定（c4_final_pk）、每个决定恰有一条
                // 工作（c4_work_decision_uq），且工作 action 与决定种类由同一写端绑定；
                // 因此“无相反动作”已由这三项共同保证，不另加第二套检查。
                lock_delivery(writer.connection(), input.delivery_id).await?;
                let now = database_now(writer.connection()).await?;
                if let Some(start) = load_start(writer.connection(), input.work_id).await? {
                    return Ok(StartOutcome::AlreadyCommitted(StartReceipt::started(
                        CommandId::from_uuid(start.command_id),
                        input.final_decision_id,
                        input.work_id,
                        start.attempt_id,
                        start.fence,
                    )));
                }
                let attempt = load_attempt(writer.connection(), input.attempt_id)
                    .await?
                    .ok_or_else(|| conflict(DepositBusinessRejection::FenceMismatch))?;
                if attempt.work_id != input.work_id
                    || attempt.delivery_id != input.delivery_id
                    || attempt.fence != input.fence
                    || attempt.expected_digest != input.expected_manifest_digest
                {
                    return Err(conflict(DepositBusinessRejection::FenceMismatch));
                }
                let session_binding: Vec<u8> = sqlx::query_scalar(
                    "SELECT binding_digest FROM public.mail_evidence_sessions WHERE id = $1",
                )
                .bind(attempt.session_id)
                .fetch_one(writer.connection())
                .await?;
                if session_binding.as_slice() != input.session_binding_digest.as_slice() {
                    return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
                }
                let delivery = sqlx::query(
                    "SELECT status, lease_token, fencing_version, attempt_count,
                        COALESCE(lease_expires_at > clock_timestamp(), false) AS lease_active
                 FROM public.deliveries WHERE id = $1 FOR UPDATE",
                )
                .bind(input.delivery_id)
                .fetch_one(writer.connection())
                .await?;
                let status: String = delivery.try_get("status")?;
                let lease_token: Option<Uuid> = delivery.try_get("lease_token")?;
                let fencing: i64 = delivery.try_get("fencing_version")?;
                let lease_active: bool = delivery.try_get("lease_active")?;
                if status != "claimed"
                    || lease_token != Some(input.claim_token)
                    || u64::try_from(fencing).ok() != Some(input.fence)
                    || !lease_active
                {
                    return Err(conflict(DepositBusinessRejection::FenceMismatch));
                }
                if input.action == DispositionAction::Take
                    && take_season_closed(writer.connection(), input.final_decision_id, now).await?
                {
                    return Err(conflict(DepositBusinessRejection::SeasonClosed));
                }
                let command_id = writer.command_id();
                insert_start(
                    writer.connection(),
                    input.work_id,
                    input.final_decision_id,
                    input.delivery_id,
                    input.attempt_id,
                    input.fence,
                    command_id,
                    now,
                )
                .await?;
                let receipt = StartReceipt::started(
                    input.command_id,
                    input.final_decision_id,
                    input.work_id,
                    input.attempt_id,
                    input.fence,
                );
                let _ = writer.finish(receipt.clone(), now).await?;
                Ok(StartOutcome::FirstCommitted(receipt))
            })
        })
        .await;
    transaction.map_err(WriteError::into_application)?.into_result()
}

enum StartOutcome {
    FirstCommitted(StartReceipt),
    AlreadyCommitted(StartReceipt),
}

impl StartOutcome {
    fn into_result(self) -> Result<WriteStartOutcome, ConsignmentError> {
        Ok(match self {
            Self::FirstCommitted(receipt) => WriteStartOutcome::FirstCommitted(receipt),
            Self::AlreadyCommitted(receipt) => WriteStartOutcome::AlreadyCommitted(receipt),
        })
    }
}

async fn apply_port(
    repository: &DepositRepository,
    command: auction_application::consignment::ApplyMailProofCommand,
) -> Result<ProofReceipt, ConsignmentError> {
    let mut pre = repository
        .store()
        .pool()
        .acquire()
        .await
        .map_err(|error| WriteError::from(error).into_application())?;
    let attempt = load_attempt(&mut pre, command.original_attempt_id)
        .await
        .map_err(WriteError::into_application)?
        .ok_or(ConsignmentError::Deposit(DepositBusinessRejection::FenceMismatch))?;
    let (key, sellers, equipment) = pre_read_work_connection(&mut pre, attempt.work_id)
        .await
        .map_err(WriteError::into_application)?;
    drop(pre);
    let keys = LockKeys {
        command_id: command.command_id.as_uuid(),
        sellers: &sellers,
        equipment: &equipment,
    };
    let max_open_per_user = repository.settings().max_open_per_user;
    let content = Arc::clone(repository.dispatch_content());
    let mail_keys = [key.clone()];
    let input = command;
    let transaction =
        with_deposit_transaction(repository.store(), keys, &mail_keys, &input, |input, tx| {
            let sellers = sellers.clone();
            let equipment = equipment.clone();
            let key = key.clone();
            let content = Arc::clone(&content);
            Box::pin(async move {
                tx.ensure_lock_set(&sellers, &equipment, std::slice::from_ref(&key))?;
                let arbitration = tx
                    .arbitrate(
                        DepositActor::Worker,
                        DepositOperation::ApplyMailProof,
                        DepositTarget::OriginalAttempt(input.original_attempt_id),
                        input.request_hash,
                        None,
                    )
                    .await?;
                let mut writer = match arbitration {
                    DepositArbitration::Replay(stored) => {
                        return match replay_receipt::<ProofReceipt>(&stored)? {
                            Outcome::Accepted(receipt) => Ok(Outcome::Accepted(receipt)),
                            Outcome::Rejected(_) => {
                                Err(invariant("consignment_proof_replay_rejected"))
                            }
                        };
                    }
                    DepositArbitration::First(writer) => writer,
                };
                let attempt = load_attempt(writer.connection(), input.original_attempt_id)
                    .await?
                    .ok_or_else(|| conflict(DepositBusinessRejection::FenceMismatch))?;
                let work = load_work_for_update(writer.connection(), attempt.work_id)
                    .await?
                    .ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
                sqlx::query("SELECT id FROM public.deliveries WHERE id = $1 FOR UPDATE")
                    .bind(work.delivery_id)
                    .fetch_one(writer.connection())
                    .await?;
                let now = database_now(writer.connection()).await?;
                if work.decision_id != input.final_decision_id
                    || work.delivery_id != input.delivery_id
                    || work.action != input.action
                    || attempt.delivery_id != input.delivery_id
                    || attempt.fence != input.fence
                {
                    return Err(conflict(DepositBusinessRejection::FenceMismatch));
                }
                let start = load_start(writer.connection(), work.work_id)
                    .await?
                    .ok_or_else(|| conflict(DepositBusinessRejection::FenceMismatch))?;
                if start.attempt_id != input.original_attempt_id
                    || start.fence != input.fence
                    || start.delivery_id != input.delivery_id
                    || start.decision_id != Some(input.final_decision_id)
                {
                    return Err(conflict(DepositBusinessRejection::FenceMismatch));
                }
                if proof_kind_action(input.proof_kind) != input.action {
                    return Err(conflict(DepositBusinessRejection::SourceMismatch));
                }
                if let Some(existing) = existing_proof_receipt(
                    writer.connection(),
                    work.work_id,
                    input.command_id,
                    input.final_decision_id,
                )
                .await?
                    && existing.disposition
                        != auction_application::consignment::ReceiptDisposition::EvidenceQuarantined
                {
                    let expected = match input.action {
                        DispositionAction::Take => {
                            auction_application::consignment::ReceiptDisposition::TakeProven
                        }
                        DispositionAction::Return => {
                            auction_application::consignment::ReceiptDisposition::ReturnProven
                        }
                    };
                    if existing.disposition != expected {
                        return Err(conflict(DepositBusinessRejection::EvidenceConflict));
                    }
                    if let Some((digest, observation)) =
                        load_proof_identity(writer.connection(), work.work_id).await?
                        && (digest != input.proof_digest || observation != input.observation_id)
                    {
                        return Err(conflict(DepositBusinessRejection::EvidenceConflict));
                    }
                    let _ = writer.finish(existing.clone(), now).await?;
                    return Ok(Outcome::Accepted(existing));
                }
                let sufficient = evidence_sufficient(
                    writer.connection(),
                    &key,
                    input.observation_id,
                    &input.validator_version,
                    input.proof_digest,
                )
                .await?;
                let conclusion = if sufficient {
                    if input.action == DispositionAction::Take {
                        ProofDisposition::TakeProven
                    } else {
                        ProofDisposition::ReturnProven
                    }
                } else {
                    ProofDisposition::EvidenceQuarantined
                };
                let proof_id = Uuid::new_v4();
                let proof_observation = if sufficient {
                    input.observation_id
                } else {
                    let exists = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM public.mail_envelope_observations WHERE id = $1)",
                )
                .bind(input.observation_id)
                .fetch_one(writer.connection())
                .await?;
                    if exists { input.observation_id } else { work.expected_observation_id }
                };
                let (proof_id, proof_conclusion) = upsert_proof(
                    writer.connection(),
                    &ProofWrite {
                        proof_id,
                        work_id: work.work_id,
                        decision_id: work.decision_id,
                        delivery_id: work.delivery_id,
                        fence: start.fence,
                        attempt_id: start.attempt_id,
                        kind: input.proof_kind,
                        conclusion,
                        observation_id: proof_observation,
                        validator_version: &input.validator_version,
                        digest: input.proof_digest,
                        now,
                    },
                )
                .await?;
                let mut receipt = ProofReceipt::new(
                    input.command_id,
                    proof_conclusion,
                    input.final_decision_id,
                    proof_id,
                );
                if proof_conclusion == ProofDisposition::TakeProven {
                    let changes = crate::database::consignment::deposit::apply_take_custody_in_tx(
                        &mut writer,
                        input.final_decision_id,
                        proof_id,
                        now,
                        max_open_per_user,
                        &content,
                    )
                    .await?;
                    receipt.listing_changes = writer.listing_changes();
                    receipt.custody_changes = changes;
                    if receipt.custody_changes.is_empty() {
                        return Err(invariant("consignment_proof_custody_count"));
                    }
                }
                let _ = writer.finish(receipt.clone(), now).await?;
                Ok(Outcome::Accepted(receipt))
            })
        })
        .await;
    transaction.map_err(WriteError::into_application)?.into_result()
}

async fn lock_delivery(connection: &mut PgConnection, delivery_id: Uuid) -> Result<(), WriteError> {
    let locked: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM public.deliveries WHERE id = $1 FOR UPDATE")
            .bind(delivery_id)
            .fetch_optional(connection)
            .await?;
    locked.ok_or_else(|| invariant("consignment_work_delivery"))?;
    Ok(())
}

fn proof_kind_action(kind: MailProofKind) -> DispositionAction {
    match kind {
        MailProofKind::ExactTake => DispositionAction::Take,
        MailProofKind::ExactReturn => DispositionAction::Return,
    }
}

async fn evidence_sufficient(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    observation_id: Uuid,
    validator_version: &str,
    digest: [u8; 32],
) -> Result<bool, WriteError> {
    if validator_version.is_empty() || digest == [0; 32] {
        return Ok(false);
    }
    let columns = MailKeyColumns::from_key(key);
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM public.mail_envelope_observations
            WHERE bot_uid = $1 AND world_id = $2 AND external_mid = $3 AND id = $4
         )",
    )
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .bind(observation_id)
    .fetch_one(connection)
    .await?;
    Ok(exists)
}

async fn take_season_closed(
    connection: &mut PgConnection,
    decision_id: Uuid,
    now: Timestamp,
) -> Result<bool, WriteError> {
    let closed: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1
            FROM public.consignment_deposit_items AS item
            JOIN public.consignment_custody_positions AS position ON position.item_id = item.id
            JOIN public.consignment_season_windows AS season_window
              ON season_window.id = position.season_window_id
            WHERE item.decision_id = $1 AND position.world_id = 'isekai'
              AND season_window.end_utc <= $2
         )",
    )
    .bind(decision_id)
    .bind(now)
    .fetch_one(connection)
    .await?;
    Ok(closed)
}

async fn pre_read_work(
    repository: &DepositRepository,
    work_id: Uuid,
) -> Result<
    (OriginalMailKey, Vec<Uuid>, Vec<auction_domain::consignment::ListingEquipmentIdentity>),
    ConsignmentError,
> {
    let mut connection = repository
        .store()
        .pool()
        .acquire()
        .await
        .map_err(|error| WriteError::from(error).into_application())?;
    let result = pre_read_work_connection(&mut connection, work_id)
        .await
        .map_err(WriteError::into_application)?;
    Ok(result)
}

async fn pre_read_work_connection(
    connection: &mut PgConnection,
    work_id: Uuid,
) -> Result<
    (OriginalMailKey, Vec<Uuid>, Vec<auction_domain::consignment::ListingEquipmentIdentity>),
    WriteError,
> {
    let row = sqlx::query(
        "SELECT w.bot_uid, w.world_id, w.external_mid, w.decision_id, w.action
         FROM public.mail_disposition_work AS w WHERE w.id = $1",
    )
    .bind(work_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| invariant("consignment_work_missing"))?;
    let world: String = row.try_get("world_id")?;
    let key = OriginalMailKey::new(
        &row.try_get::<String, _>("bot_uid")?,
        parse_world(&world)?,
        &row.try_get::<String, _>("external_mid")?,
    )
    .map_err(invariant_decoding("consignment_work_mail_key"))?;
    let decision_id: Uuid = row.try_get("decision_id")?;
    let action: String = row.try_get("action")?;
    if action != "take" {
        return Ok((key, Vec::new(), Vec::new()));
    }
    let rows = sqlx::query(
        "SELECT item.seller_user_id, item.world_id, item.canonical_eid
         FROM public.consignment_deposit_items AS item
         WHERE item.decision_id = $1 ORDER BY item.ordinal",
    )
    .bind(decision_id)
    .fetch_all(&mut *connection)
    .await?;
    let sellers: Vec<Uuid> =
        rows.iter().map(|row| row.try_get("seller_user_id")).collect::<Result<_, _>>()?;
    let equipment = rows
        .iter()
        .map(|row| {
            auction_domain::consignment::ListingEquipmentIdentity::new(
                &row.try_get::<String, _>("world_id")?,
                &row.try_get::<String, _>("canonical_eid")?,
            )
            .map_err(invariant_decoding("consignment_work_equipment"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((key, sellers, equipment))
}
