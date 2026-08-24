//! PostgreSQL recovery case 到期推进适配器。
//!
//! 该 owner 只负责锁定 recovery case、观察已经持久化的结算履约事实并创建下一条可审计
//! action。它不读取外部凭据，也不直接发送 MoogleMail；外部动作必须在后续 delivery owner
//! 提交后执行。

use auction_application::{
    ApplicationError, ApplicationFuture, RecoveryActionClaim, RecoveryActionOutcome,
    RecoveryCaseAdvance, RecoveryReturnAdvance, RecoveryWorkStore, RequestHash,
};
use auction_domain::CommandId;
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde::Deserialize;
#[cfg(feature = "postgres")]
use serde_json::Value;
use uuid::Uuid;

#[cfg(feature = "postgres")]
use auction_domain::{SettlementId, deterministic_uuid};
#[cfg(feature = "postgres")]
use sha2::{Digest, Sha256};
#[cfg(feature = "postgres")]
use sqlx::{Postgres, Row};

use super::DatabaseStore;

impl RecoveryWorkStore for DatabaseStore {
    fn record_due_return(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryReturnAdvance>> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(repository_error("recovery 退回邮件记录时间无效").into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    record_return_postgres(store.pool(), now).await.map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery 退回邮件 Worker 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn advance_due_case(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryCaseAdvance>> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(repository_error("recovery Worker 时间无效").into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    advance_postgres(store.pool(), now).await.map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery Worker 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn claim_due_action(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryActionClaim>> {
        Box::pin(async move {
            if worker_id.is_empty()
                || worker_id != worker_id.trim()
                || now.timestamp_nanos_opt().is_none()
                || lease_until <= now
            {
                return Err(repository_error("recovery action 领取参数无效").into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    claim_action_postgres(store.pool(), &worker_id, now, lease_until)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery action Worker 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn link_action_preparation(
        &self,
        claim: RecoveryActionClaim,
        delivery_id: Uuid,
        command_id: CommandId,
        request_hash: RequestHash,
        linked_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, RecoveryActionClaim> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.execution_state != "pending"
                || claim.claim_token.is_nil()
                || delivery_id.is_nil()
                || command_id.is_nil()
                || request_hash.as_bytes() == [0; 32]
                || linked_at.timestamp_nanos_opt().is_none()
            {
                return Err(repository_error("recovery action prepared 关联参数无效").into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => link_action_postgres(
                    store.pool(),
                    &claim,
                    delivery_id,
                    command_id,
                    request_hash,
                    linked_at,
                )
                .await
                .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery action Worker 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn mark_action_enqueued(
        &self,
        claim: RecoveryActionClaim,
        activate_command_id: CommandId,
        activate_request_hash: RequestHash,
        activated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, RecoveryActionClaim> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.execution_state != "prepared"
                || claim.claim_token.is_nil()
                || claim.delivery_id.is_none_or(|value| value.is_nil())
                || claim.prepare_command_id.is_none_or(CommandId::is_nil)
                || claim.prepare_request_hash.is_none_or(|value| value.as_bytes() == [0; 32])
                || activate_command_id.is_nil()
                || activate_request_hash.as_bytes() == [0; 32]
                || activated_at.timestamp_nanos_opt().is_none()
            {
                return Err(repository_error("recovery action activation 参数无效").into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => mark_action_enqueued_postgres(
                    store.pool(),
                    &claim,
                    activate_command_id,
                    activate_request_hash,
                    activated_at,
                )
                .await
                .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery action Worker 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn mark_action_satisfied(
        &self,
        claim: RecoveryActionClaim,
        proof_kind: String,
        proof_reference: String,
        observed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.buyer_user_id.is_nil()
                || claim.kind != "buyer_blacklist"
                || claim.execution_state != "pending"
                || claim.delivery_id.is_some()
                || claim.claim_token.is_nil()
                || observed_at < claim.created_at
                || observed_at.timestamp_nanos_opt().is_none()
                || !valid_failure_code(&proof_kind)
                || !valid_failure_code(&proof_reference)
            {
                return Err(repository_error("recovery action 完成证明参数无效").into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => mark_action_satisfied_postgres(
                    store.pool(),
                    &claim,
                    &proof_kind,
                    &proof_reference,
                    observed_at,
                )
                .await
                .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery action Worker 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn observe_action(
        &self,
        claim: RecoveryActionClaim,
        observed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, RecoveryActionOutcome> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.execution_state != "enqueued"
                || claim.claim_token.is_nil()
                || claim.delivery_id.is_none_or(|value| value.is_nil())
                || observed_at.timestamp_nanos_opt().is_none()
            {
                return Err(repository_error("recovery action 观察参数无效").into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => observe_action_postgres(store.pool(), &claim, observed_at)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery action Worker 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn release_action(
        &self,
        claim: RecoveryActionClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.claim_token.is_nil()
                || retry_at.timestamp_nanos_opt().is_none()
                || !valid_failure_code(&failure_code)
            {
                return Err(repository_error("recovery action 释放参数无效").into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    release_action_postgres(store.pool(), &claim, retry_at, &failure_code)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery action Worker 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn mark_action_manual_reconcile(
        &self,
        claim: RecoveryActionClaim,
        failure_code: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.claim_token.is_nil()
                || !valid_failure_code(&failure_code)
            {
                return Err(repository_error("recovery action 人工复核参数无效").into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    manual_action_postgres(store.pool(), &claim, &failure_code)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery action Worker 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

fn repository_error(message: &str) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(message.to_owned())
}

#[cfg(feature = "postgres")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryReturnMetadata {
    obligation_id: String,
    credits: i64,
    cod: i64,
    attachment_ids: Vec<String>,
}

#[cfg(feature = "postgres")]
#[derive(Debug)]
struct RecoveryReturnRow {
    recovery_id: Uuid,
    return_mail_id: Uuid,
    return_external_mail_id: String,
    return_world_id: String,
    return_metadata: Value,
    return_received_at: DateTime<Utc>,
    returned_obligation_id: Uuid,
    returned_obligation_kind: String,
    returned_carrier_credit: i64,
    external_equipment_id: String,
    recovery_world_id: String,
    payment_obligation_id: Uuid,
    equipment_obligation_id: Option<Uuid>,
    state_version: i64,
    created_at: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
async fn record_return_postgres(
    pool: &super::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<RecoveryReturnAdvance>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;

    let Some(row) = sqlx::query(
        r#"SELECT recovery.id AS recovery_id,
                  inbound.id AS return_mail_id,
                  inbound.external_mail_id AS return_external_mail_id,
                  inbound.world_id AS return_world_id,
                  inbound.decision_metadata AS return_metadata,
                  inbound.received_at AS return_received_at,
                  obligation.id AS returned_obligation_id,
                  obligation.obligation_kind AS returned_obligation_kind,
                  obligation.carrier_credit AS returned_carrier_credit,
                  equipment.external_equipment_id,
                  recovery.world_id AS recovery_world_id,
                  recovery.payment_obligation_id,
                  recovery.equipment_obligation_id,
                  recovery.state_version,
                  recovery.created_at
           FROM public.settlement_recoveries AS recovery
           INNER JOIN public.inbound_mail AS inbound
               ON inbound.decision_metadata ->> 'obligation_id' IN (
                   recovery.payment_obligation_id::text,
                   recovery.equipment_obligation_id::text
               )
           INNER JOIN public.settlement_obligations AS obligation
               ON obligation.id::text = inbound.decision_metadata ->> 'obligation_id'
           INNER JOIN public.equipment AS equipment
               ON equipment.id = recovery.equipment_id
           WHERE recovery.return_mail_id IS NULL
             AND recovery.stage <> 'completed'
             AND inbound.mail_kind = 'delivery'
             AND inbound.status = 'accepted'
             AND inbound.decision_code = 'settlement_recovery_returned'
             AND inbound.received_at <= $1
             AND NOT EXISTS (
                 SELECT 1
                 FROM public.settlement_recoveries AS consumed
                 WHERE consumed.return_mail_id = inbound.id
             )
           ORDER BY inbound.received_at, inbound.id, recovery.id
           LIMIT 1
           FOR UPDATE OF recovery SKIP LOCKED"#,
    )
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .map(map_recovery_return_row)
    .transpose()?
    else {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(None);
    };

    if row.return_received_at < row.created_at || row.return_received_at > now {
        return Err(repository_error("recovery 退回邮件时间越界"));
    }
    if !valid_external_reference(&row.return_external_mail_id)
        || row.recovery_id.is_nil()
        || row.return_mail_id.is_nil()
        || row.returned_obligation_id.is_nil()
        || row.payment_obligation_id.is_nil()
        || row.equipment_obligation_id.is_some_and(|value| value.is_nil())
    {
        return Err(repository_error("recovery 退回邮件持久化事实无效"));
    }

    let metadata: RecoveryReturnMetadata = serde_json::from_value(row.return_metadata.clone())
        .map_err(|_| repository_error("recovery 退回邮件 metadata 不是严格的证据对象"))?;
    validate_recovery_return_metadata(&metadata)?;
    if metadata.obligation_id != row.returned_obligation_id.to_string() {
        return Err(repository_error("recovery 退回邮件 obligation 引用冲突"));
    }

    let expected = expected_recovery_return(&row);
    let matches = expected.as_ref().is_some_and(|expected| {
        metadata.credits == expected.expected_credits
            && metadata.cod == expected.expected_cod
            && metadata.attachment_ids == expected.expected_attachment_ids
    });
    let blocking_code = (!matches).then(|| "evidence_mismatch".to_owned());

    let updated = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET return_mail_id = $1,
               return_external_mail_id = $2,
               stage = 'reclaiming',
               blocking_code = $3,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $4
           WHERE id = $5
             AND state_version = $6
             AND return_mail_id IS NULL
             AND stage <> 'completed'
           RETURNING id, stage, state_version"#,
    )
    .bind(row.return_mail_id)
    .bind(&row.return_external_mail_id)
    .bind(&blocking_code)
    .bind(now)
    .bind(row.recovery_id)
    .bind(row.state_version)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("recovery 退回邮件记录 fence 已失效"))?;

    let mut action_kinds = Vec::new();
    if let Some(expected) = expected.filter(|_| blocking_code.is_none()) {
        insert_recovery_action(
            &mut transaction,
            RecoveryActionInsert {
                recovery_id: row.recovery_id,
                kind: expected.kind,
                ordinal: 1,
                source_mail_id: Some(row.return_mail_id),
                expected_credits: expected.expected_credits,
                expected_cod: expected.expected_cod,
                expected_attachment_ids: &expected.expected_attachment_ids,
                now,
            },
        )
        .await?;
        action_kinds.push(expected.kind.to_owned());

        if matches!(expected.kind, "reclaim_equipment_take" | "cancel_carrier_take") {
            insert_recovery_action(
                &mut transaction,
                RecoveryActionInsert {
                    recovery_id: row.recovery_id,
                    kind: "buyer_blacklist",
                    ordinal: 2,
                    source_mail_id: None,
                    expected_credits: 0,
                    expected_cod: 0,
                    expected_attachment_ids: &[],
                    now,
                },
            )
            .await?;
            action_kinds.push("buyer_blacklist".to_owned());
        }
    }

    let recovery_id: Uuid = updated.try_get("id").map_err(map_storage_error)?;
    let stage: String = updated.try_get("stage").map_err(map_storage_error)?;
    let state_version: i64 = updated.try_get("state_version").map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(Some(RecoveryReturnAdvance {
        recovery_id: auction_domain::SettlementId::from_uuid(recovery_id),
        return_mail_id: row.return_mail_id,
        stage,
        blocking_code,
        action_kinds,
        state_version,
        recorded_at: now,
    }))
}

#[cfg(feature = "postgres")]
#[derive(Debug)]
struct ExpectedRecoveryReturn {
    kind: &'static str,
    expected_credits: i64,
    expected_cod: i64,
    expected_attachment_ids: Vec<String>,
}

#[cfg(feature = "postgres")]
fn expected_recovery_return(row: &RecoveryReturnRow) -> Option<ExpectedRecoveryReturn> {
    match (
        row.recovery_world_id.as_str(),
        row.return_world_id.as_str(),
        row.returned_obligation_id,
        row.returned_obligation_kind.as_str(),
    ) {
        ("main", "main", obligation_id, "winner_equipment_cod")
            if obligation_id == row.payment_obligation_id
                && valid_recovery_text(&row.external_equipment_id, 128) =>
        {
            Some(ExpectedRecoveryReturn {
                kind: "reclaim_equipment_take",
                expected_credits: 0,
                expected_cod: 0,
                expected_attachment_ids: vec![row.external_equipment_id.clone()],
            })
        }
        ("isekai", "main", obligation_id, "winner_payment_carrier_cod")
            if obligation_id == row.payment_obligation_id && row.returned_carrier_credit > 0 =>
        {
            Some(ExpectedRecoveryReturn {
                kind: "cancel_carrier_take",
                expected_credits: row.returned_carrier_credit,
                expected_cod: 0,
                expected_attachment_ids: Vec::new(),
            })
        }
        ("isekai", "isekai", obligation_id, "winner_equipment_handoff")
            if row.equipment_obligation_id == Some(obligation_id)
                && valid_recovery_text(&row.external_equipment_id, 128) =>
        {
            Some(ExpectedRecoveryReturn {
                kind: "recover_equipment_take",
                expected_credits: 0,
                expected_cod: 0,
                expected_attachment_ids: vec![row.external_equipment_id.clone()],
            })
        }
        _ => None,
    }
}

#[cfg(feature = "postgres")]
fn validate_recovery_return_metadata(
    metadata: &RecoveryReturnMetadata,
) -> Result<(), auction_application::RepositoryError> {
    let obligation_id = Uuid::parse_str(&metadata.obligation_id)
        .map_err(|_| repository_error("recovery 退回邮件 obligation ID 无效"))?;
    if obligation_id.is_nil()
        || obligation_id.to_string() != metadata.obligation_id
        || metadata.credits < 0
        || metadata.cod < 0
        || metadata.attachment_ids.len() > 100
        || metadata.attachment_ids.iter().enumerate().any(|(index, value)| {
            !valid_recovery_text(value, 128)
                || index > 0 && value <= &metadata.attachment_ids[index - 1]
        })
    {
        return Err(repository_error("recovery 退回邮件 metadata 越界或未排序"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
struct RecoveryActionInsert<'a> {
    recovery_id: Uuid,
    kind: &'a str,
    ordinal: i32,
    source_mail_id: Option<Uuid>,
    expected_credits: i64,
    expected_cod: i64,
    expected_attachment_ids: &'a [String],
    now: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
async fn insert_recovery_action(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    spec: RecoveryActionInsert<'_>,
) -> Result<(), auction_application::RepositoryError> {
    let action_id = deterministic_uuid(
        CommandId::from_uuid(spec.recovery_id),
        &format!("recovery-action:{}", spec.kind),
        1,
    );
    sqlx::query(
        r#"INSERT INTO public.settlement_recovery_actions
               (id, recovery_id, kind, ordinal, execution_state, state_version,
                source_mail_id, expected_credits, expected_cod, expected_attachment_ids,
                created_at, updated_at)
           VALUES ($1, $2, $3, $4, 'pending', 1, $5, $6, $7, $8, $9, $9)
           ON CONFLICT (recovery_id, kind) DO NOTHING"#,
    )
    .bind(action_id)
    .bind(spec.recovery_id)
    .bind(spec.kind)
    .bind(spec.ordinal)
    .bind(spec.source_mail_id)
    .bind(spec.expected_credits)
    .bind(spec.expected_cod)
    .bind(spec.expected_attachment_ids)
    .bind(spec.now)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;

    let row = sqlx::query(
        r#"SELECT id, ordinal, source_mail_id, expected_credits, expected_cod,
                  expected_attachment_ids
           FROM public.settlement_recovery_actions
           WHERE recovery_id = $1 AND kind = $2
           FOR UPDATE"#,
    )
    .bind(spec.recovery_id)
    .bind(spec.kind)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let stored_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
    let stored_ordinal: i32 = row.try_get("ordinal").map_err(map_storage_error)?;
    let stored_source: Option<Uuid> = row.try_get("source_mail_id").map_err(map_storage_error)?;
    let stored_credits: i64 = row.try_get("expected_credits").map_err(map_storage_error)?;
    let stored_cod: i64 = row.try_get("expected_cod").map_err(map_storage_error)?;
    let stored_attachments: Vec<String> =
        row.try_get("expected_attachment_ids").map_err(map_storage_error)?;
    if stored_id != action_id
        || stored_ordinal != spec.ordinal
        || stored_source != spec.source_mail_id
        || stored_credits != spec.expected_credits
        || stored_cod != spec.expected_cod
        || stored_attachments != spec.expected_attachment_ids
    {
        return Err(repository_error("recovery action 证据内容冲突"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn map_recovery_return_row(
    row: sqlx::postgres::PgRow,
) -> Result<RecoveryReturnRow, auction_application::RepositoryError> {
    Ok(RecoveryReturnRow {
        recovery_id: row.try_get("recovery_id").map_err(map_storage_error)?,
        return_mail_id: row.try_get("return_mail_id").map_err(map_storage_error)?,
        return_external_mail_id: row
            .try_get("return_external_mail_id")
            .map_err(map_storage_error)?,
        return_world_id: row.try_get("return_world_id").map_err(map_storage_error)?,
        return_metadata: row.try_get("return_metadata").map_err(map_storage_error)?,
        return_received_at: row.try_get("return_received_at").map_err(map_storage_error)?,
        returned_obligation_id: row.try_get("returned_obligation_id").map_err(map_storage_error)?,
        returned_obligation_kind: row
            .try_get("returned_obligation_kind")
            .map_err(map_storage_error)?,
        returned_carrier_credit: row
            .try_get("returned_carrier_credit")
            .map_err(map_storage_error)?,
        external_equipment_id: row.try_get("external_equipment_id").map_err(map_storage_error)?,
        recovery_world_id: row.try_get("recovery_world_id").map_err(map_storage_error)?,
        payment_obligation_id: row.try_get("payment_obligation_id").map_err(map_storage_error)?,
        equipment_obligation_id: row
            .try_get("equipment_obligation_id")
            .map_err(map_storage_error)?,
        state_version: row.try_get("state_version").map_err(map_storage_error)?,
        created_at: row.try_get("created_at").map_err(map_storage_error)?,
    })
}

#[cfg(feature = "postgres")]
#[derive(Debug)]
struct DueRecoveryRow {
    id: Uuid,
    stage: String,
    state_version: i64,
    reminder_count: i32,
    reminder_limit: i32,
    world_id: String,
    payment_execution_state: Option<String>,
    payment_completed_at: Option<DateTime<Utc>>,
    equipment_execution_state: Option<String>,
    equipment_completed_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
async fn advance_postgres(
    pool: &super::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<RecoveryCaseAdvance>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;

    let Some(row) = sqlx::query(
        r#"SELECT recovery.id,
                  recovery.stage,
                  recovery.state_version,
                  recovery.reminder_count,
                  recovery.reminder_limit,
                  recovery.world_id,
                  payment_f.execution_state AS payment_execution_state,
                  payment_f.completed_at AS payment_completed_at,
                  equipment_f.execution_state AS equipment_execution_state,
                  equipment_f.completed_at AS equipment_completed_at,
                  recovery.created_at
           FROM public.settlement_recoveries AS recovery
           LEFT JOIN public.settlement_obligation_fulfillments AS payment_f
               ON payment_f.obligation_id = recovery.payment_obligation_id
           LEFT JOIN public.settlement_obligation_fulfillments AS equipment_f
               ON equipment_f.obligation_id = recovery.equipment_obligation_id
           WHERE (recovery.next_attempt_at IS NULL OR recovery.next_attempt_at <= $1)
             AND (
                 (
                     recovery.stage IN ('awaiting_payment', 'payment_overdue')
                     AND payment_f.execution_state = 'satisfied'
                 )
                 OR (
                     recovery.stage = 'awaiting_payment'
                     AND recovery.payment_deadline_at <= $1
                 )
                 OR (
                     recovery.world_id = 'isekai'
                     AND recovery.stage = 'delivering_equipment'
                     AND equipment_f.execution_state = 'satisfied'
                 )
             )
           ORDER BY COALESCE(recovery.next_attempt_at, recovery.payment_deadline_at),
                    recovery.created_at,
                    recovery.id
           LIMIT 1
           FOR UPDATE OF recovery SKIP LOCKED"#,
    )
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .map(map_due_recovery_row)
    .transpose()?
    else {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(None);
    };

    let payment_satisfied = row.payment_execution_state.as_deref() == Some("satisfied");
    let equipment_satisfied = row.equipment_execution_state.as_deref() == Some("satisfied");
    let (next_stage, next_reminder_count, completed_at, create_reminder) =
        if row.world_id == "main" && payment_satisfied {
            ("completed", row.reminder_count, row.payment_completed_at, false)
        } else if row.world_id == "isekai" && equipment_satisfied {
            ("completed", row.reminder_count, row.equipment_completed_at, false)
        } else if row.world_id == "isekai" && payment_satisfied {
            ("delivering_equipment", row.reminder_count, None, false)
        } else if row.stage == "awaiting_payment" {
            if row.reminder_count >= row.reminder_limit {
                return Err(repository_error("recovery 提醒次数已超过冻结上限"));
            }
            (
                "payment_overdue",
                row.reminder_count
                    .checked_add(1)
                    .ok_or_else(|| repository_error("recovery 提醒次数溢出"))?,
                None,
                true,
            )
        } else {
            return Err(repository_error("recovery due case 状态不支持推进"));
        };

    if next_stage == "completed" {
        let Some(completed_at) = completed_at else {
            return Err(repository_error("recovery 完成事实缺少履约时间"));
        };
        if completed_at < row.created_at || completed_at > now {
            return Err(repository_error("recovery 完成事实时间越界"));
        }
    }

    let updated = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET stage = $1,
               reminder_count = $2,
               state_version = state_version + 1,
               next_attempt_at = NULL,
               completed_at = $3,
               updated_at = $4
           WHERE id = $5
             AND stage = $6
             AND state_version = $7
           RETURNING id, stage, reminder_count, state_version"#,
    )
    .bind(next_stage)
    .bind(next_reminder_count)
    .bind(completed_at)
    .bind(now)
    .bind(row.id)
    .bind(&row.stage)
    .bind(row.state_version)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("recovery case 状态 fence 已失效"))?;

    if create_reminder {
        let action_id =
            deterministic_uuid(CommandId::from_uuid(row.id), "recovery-action:payment-reminder", 1);
        sqlx::query(
            r#"INSERT INTO public.settlement_recovery_actions
                   (id, recovery_id, kind, ordinal, execution_state, state_version,
                    created_at, updated_at)
               VALUES ($1, $2, 'payment_reminder', 0, 'pending', 1, $3, $3)
               ON CONFLICT (recovery_id, kind) DO NOTHING"#,
        )
        .bind(action_id)
        .bind(row.id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;

        let stored_action: Uuid = sqlx::query(
            "SELECT id FROM public.settlement_recovery_actions
             WHERE recovery_id = $1 AND kind = 'payment_reminder' FOR UPDATE",
        )
        .bind(row.id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(map_storage_error)?
        .try_get("id")
        .map_err(map_storage_error)?;
        if stored_action != action_id {
            return Err(repository_error("recovery 提醒 action 标识冲突"));
        }
    }

    let recovery_id: Uuid = updated.try_get("id").map_err(map_storage_error)?;
    let stage: String = updated.try_get("stage").map_err(map_storage_error)?;
    let reminder_count: i32 = updated.try_get("reminder_count").map_err(map_storage_error)?;
    let state_version: i64 = updated.try_get("state_version").map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(Some(RecoveryCaseAdvance {
        recovery_id: SettlementId::from_uuid(recovery_id),
        completed: stage == "completed",
        stage,
        reminder_count,
        state_version,
        advanced_at: now,
    }))
}

#[cfg(feature = "postgres")]
async fn claim_action_postgres(
    pool: &super::TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
) -> Result<Option<RecoveryActionClaim>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let Some(row) = sqlx::query(
        r#"SELECT action.id,
                  action.recovery_id,
                  action.kind,
                  action.execution_state,
                  action.delivery_id,
                  action.prepare_command_id,
                  action.prepare_request_digest,
                  action.activate_command_id,
                  action.activate_request_digest,
                  action.state_version,
                  action.claim_fencing,
                  action.retry_count,
                  action.source_mail_id,
                  action.expected_credits,
                  action.expected_cod,
                  action.expected_attachment_ids,
                  action.created_at,
                  recovery.world_id,
                  recovery.winner_user_id AS buyer_user_id,
                  recovery.payment_obligation_id,
                  source_mail.external_mail_id AS source_external_mail_id,
                  recipient.game_player_id AS recipient_game_player_id,
                  COALESCE(recipient.status = 'active', false) AS recipient_eligible,
                  COALESCE(recipient.state_version, 0) AS recipient_state_version
           FROM public.settlement_recovery_actions AS action
           INNER JOIN public.settlement_recoveries AS recovery
               ON recovery.id = action.recovery_id
           LEFT JOIN public.inbound_mail AS source_mail
               ON source_mail.id = action.source_mail_id
           LEFT JOIN public.users AS recipient
               ON recipient.id = recovery.winner_user_id
           WHERE action.kind IN (
                     'payment_reminder',
                     'reclaim_equipment_take',
                     'cancel_carrier_take',
                     'recover_equipment_take',
                     'buyer_blacklist'
                 )
             AND action.execution_state IN ('pending', 'prepared', 'enqueued')
             AND recovery.stage <> 'completed'
             AND recovery.blocking_code IS NULL
             AND (
                 action.kind <> 'buyer_blacklist'
                 OR EXISTS (
                     SELECT 1
                     FROM public.settlement_recovery_actions AS prerequisite
                     WHERE prerequisite.recovery_id = action.recovery_id
                       AND prerequisite.ordinal = 1
                       AND prerequisite.kind IN ('reclaim_equipment_take', 'cancel_carrier_take')
                       AND prerequisite.execution_state = 'satisfied'
                 )
             )
             AND (action.claim_token IS NULL OR action.claim_expires_at <= $1)
             AND (action.next_attempt_at IS NULL OR action.next_attempt_at <= $1)
           ORDER BY COALESCE(action.next_attempt_at, action.created_at),
                    action.recovery_id,
                    action.ordinal,
                    action.id
           LIMIT 1
           FOR UPDATE OF action SKIP LOCKED"#,
    )
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    else {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(None);
    };

    let action_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
    let recovery_id: Uuid = row.try_get("recovery_id").map_err(map_storage_error)?;
    let kind: String = row.try_get("kind").map_err(map_storage_error)?;
    let execution_state: String = row.try_get("execution_state").map_err(map_storage_error)?;
    let delivery_id: Option<Uuid> = row.try_get("delivery_id").map_err(map_storage_error)?;
    let prepare_command_id: Option<Uuid> =
        row.try_get("prepare_command_id").map_err(map_storage_error)?;
    let prepare_request_digest: Option<Vec<u8>> =
        row.try_get("prepare_request_digest").map_err(map_storage_error)?;
    let activate_command_id: Option<Uuid> =
        row.try_get("activate_command_id").map_err(map_storage_error)?;
    let activate_request_digest: Option<Vec<u8>> =
        row.try_get("activate_request_digest").map_err(map_storage_error)?;
    let state_version: i64 = row.try_get("state_version").map_err(map_storage_error)?;
    let recovery_world_id: String = row.try_get("world_id").map_err(map_storage_error)?;
    let buyer_user_id: Uuid = row.try_get("buyer_user_id").map_err(map_storage_error)?;
    let payment_obligation_id: Uuid =
        row.try_get("payment_obligation_id").map_err(map_storage_error)?;
    let source_mail_id: Option<Uuid> = row.try_get("source_mail_id").map_err(map_storage_error)?;
    let source_external_mail_id: Option<String> =
        row.try_get("source_external_mail_id").map_err(map_storage_error)?;
    let expected_credits: i64 = row.try_get("expected_credits").map_err(map_storage_error)?;
    let expected_cod: i64 = row.try_get("expected_cod").map_err(map_storage_error)?;
    let expected_attachment_ids: Vec<String> =
        row.try_get("expected_attachment_ids").map_err(map_storage_error)?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_storage_error)?;
    if action_id.is_nil()
        || recovery_id.is_nil()
        || buyer_user_id.is_nil()
        || !matches!(
            kind.as_str(),
            "payment_reminder"
                | "reclaim_equipment_take"
                | "cancel_carrier_take"
                | "recover_equipment_take"
                | "buyer_blacklist"
        )
        || !matches!(execution_state.as_str(), "pending" | "prepared" | "enqueued")
        || created_at > now
        || (execution_state == "enqueued" && delivery_id.is_none_or(|value| value.is_nil()))
        || (execution_state == "pending"
            && (delivery_id.is_some()
                || prepare_command_id.is_some()
                || prepare_request_digest.is_some()
                || activate_command_id.is_some()
                || activate_request_digest.is_some()))
        || (execution_state == "prepared"
            && (delivery_id.is_none_or(|value| value.is_nil())
                || prepare_command_id.is_none_or(|value| value.is_nil())
                || prepare_request_digest.as_deref().is_none_or(|value| value.len() != 32)
                || activate_command_id.is_some()
                || activate_request_digest.is_some()))
        || (execution_state == "enqueued"
            && (prepare_command_id.is_none_or(|value| value.is_nil())
                || prepare_request_digest.as_deref().is_none_or(|value| value.len() != 32)
                || activate_command_id.is_none_or(|value| value.is_nil())
                || activate_request_digest.as_deref().is_none_or(|value| value.len() != 32)))
        || !matches!(recovery_world_id.as_str(), "main" | "isekai")
        || expected_credits < 0
        || expected_cod < 0
        || expected_attachment_ids.len() > 1000
        || expected_attachment_ids.iter().any(|value| !valid_recovery_text(value, 128))
        || expected_attachment_ids.windows(2).any(|values| values[0] >= values[1])
        || (kind == "payment_reminder"
            && (source_mail_id.is_some()
                || source_external_mail_id.is_some()
                || expected_credits != 0
                || expected_cod != 0
                || !expected_attachment_ids.is_empty()))
        || (matches!(
            kind.as_str(),
            "reclaim_equipment_take" | "cancel_carrier_take" | "recover_equipment_take"
        ) && (source_mail_id.is_none()
            || source_external_mail_id
                .as_deref()
                .is_none_or(|value| !valid_external_reference(value))))
        || (kind == "buyer_blacklist"
            && (source_mail_id.is_some()
                || source_external_mail_id.is_some()
                || expected_credits != 0
                || expected_cod != 0
                || !expected_attachment_ids.is_empty()))
    {
        return Err(repository_error("recovery action 持久化事实无效"));
    }
    let world_id = match kind.as_str() {
        "payment_reminder" => "main".to_owned(),
        "cancel_carrier_take" => {
            if recovery_world_id != "isekai" {
                return Err(repository_error("recovery carrier action 世界不匹配"));
            }
            "main".to_owned()
        }
        "reclaim_equipment_take" => {
            if recovery_world_id != "main" {
                return Err(repository_error("recovery reclaim action 世界不匹配"));
            }
            "main".to_owned()
        }
        "recover_equipment_take" => {
            if recovery_world_id != "isekai" {
                return Err(repository_error("recovery equipment action 世界不匹配"));
            }
            "isekai".to_owned()
        }
        "buyer_blacklist" => recovery_world_id.clone(),
        _ => return Err(repository_error("recovery action 世界无法确定")),
    };

    let previous_fencing: i64 = row.try_get("claim_fencing").map_err(map_storage_error)?;
    let fencing_version = previous_fencing
        .checked_add(1)
        .ok_or_else(|| repository_error("recovery action fencing 版本溢出"))?;
    if fencing_version <= 0 {
        return Err(repository_error("recovery action fencing 版本无效"));
    }
    let previous_retry_count: i32 = row.try_get("retry_count").map_err(map_storage_error)?;
    let retry_count = previous_retry_count
        .checked_add(1)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| repository_error("recovery action 重试次数溢出"))?;
    let retry_count_db = i32::try_from(retry_count)
        .map_err(|_| repository_error("recovery action 重试次数超出数据库范围"))?;
    let claim_token = Uuid::new_v4();
    let updated = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET claim_token = $1,
               claimed_at = $2,
               claim_expires_at = $3,
               claim_fencing = $4,
               retry_count = $5,
               next_attempt_at = NULL,
               updated_at = $2
           WHERE id = $6
             AND execution_state = $7
             AND state_version > 0
             AND claim_fencing = $8
             AND (claim_token IS NULL OR claim_expires_at <= $2)
             AND (next_attempt_at IS NULL OR next_attempt_at <= $2)"#,
    )
    .bind(claim_token)
    .bind(now)
    .bind(lease_until)
    .bind(fencing_version)
    .bind(retry_count_db)
    .bind(action_id)
    .bind(&execution_state)
    .bind(previous_fencing)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error("recovery action 领取 fence 已失效"));
    }
    let recipient: Option<String> =
        row.try_get("recipient_game_player_id").map_err(map_storage_error)?;
    let recipient_eligible: bool = row.try_get("recipient_eligible").map_err(map_storage_error)?;
    let recipient_state_version: i64 =
        row.try_get("recipient_state_version").map_err(map_storage_error)?;
    if (recipient.is_some() && recipient_state_version <= 0) || recipient_state_version < 0 {
        return Err(repository_error("recovery 收件人 Users 状态版本无效"));
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(Some(RecoveryActionClaim {
        action_id: auction_domain::SettlementId::from_uuid(action_id),
        recovery_id: auction_domain::SettlementId::from_uuid(recovery_id),
        kind,
        world_id,
        buyer_user_id,
        recipient,
        recipient_eligible,
        recipient_state_version,
        payment_reference: payment_obligation_id.to_string(),
        source_external_mail_id,
        expected_credits,
        expected_cod,
        expected_attachment_ids,
        execution_state,
        delivery_id,
        prepare_command_id: prepare_command_id.map(CommandId::from_uuid),
        prepare_request_hash: request_hash_from_optional(prepare_request_digest)?,
        activate_command_id: activate_command_id.map(CommandId::from_uuid),
        activate_request_hash: request_hash_from_optional(activate_request_digest)?,
        claim_token,
        state_version,
        lease_until,
        fencing_version,
        retry_count,
        worker_id: worker_id.to_owned(),
        claimed_at: now,
        created_at,
    }))
}

#[cfg(feature = "postgres")]
async fn link_action_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    delivery_id: Uuid,
    command_id: CommandId,
    request_hash: RequestHash,
    linked_at: DateTime<Utc>,
) -> Result<RecoveryActionClaim, auction_application::RepositoryError> {
    let result = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET execution_state = 'prepared',
               delivery_id = $1,
               prepare_command_id = $2,
               prepare_request_digest = $3,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $4
           WHERE id = $5
             AND recovery_id = $6
             AND execution_state = 'pending'
             AND state_version > 0
             AND claim_token = $7
             AND claim_fencing = $8"#,
    )
    .bind(delivery_id)
    .bind(command_id.as_uuid())
    .bind(request_hash.as_bytes().to_vec())
    .bind(linked_at)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("recovery action delivery 关联 fence 已失效"));
    }
    let state_version = claim
        .state_version
        .checked_add(1)
        .ok_or_else(|| repository_error("recovery action 状态版本溢出"))?;
    Ok(RecoveryActionClaim {
        action_id: claim.action_id,
        recovery_id: claim.recovery_id,
        kind: claim.kind.clone(),
        world_id: claim.world_id.clone(),
        buyer_user_id: claim.buyer_user_id,
        recipient: claim.recipient.clone(),
        recipient_eligible: claim.recipient_eligible,
        recipient_state_version: claim.recipient_state_version,
        payment_reference: claim.payment_reference.clone(),
        source_external_mail_id: claim.source_external_mail_id.clone(),
        expected_credits: claim.expected_credits,
        expected_cod: claim.expected_cod,
        expected_attachment_ids: claim.expected_attachment_ids.clone(),
        execution_state: "prepared".to_owned(),
        delivery_id: Some(delivery_id),
        prepare_command_id: Some(command_id),
        prepare_request_hash: Some(request_hash),
        activate_command_id: None,
        activate_request_hash: None,
        state_version,
        claim_token: claim.claim_token,
        lease_until: claim.lease_until,
        fencing_version: claim.fencing_version,
        retry_count: claim.retry_count,
        worker_id: claim.worker_id.clone(),
        claimed_at: claim.claimed_at,
        created_at: claim.created_at,
    })
}

#[cfg(feature = "postgres")]
async fn mark_action_enqueued_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    activate_command_id: CommandId,
    activate_request_hash: RequestHash,
    activated_at: DateTime<Utc>,
) -> Result<RecoveryActionClaim, auction_application::RepositoryError> {
    let delivery_id = claim
        .delivery_id
        .ok_or_else(|| repository_error("recovery prepared action 缺少 delivery 标识"))?;
    let prepare_command_id = claim
        .prepare_command_id
        .ok_or_else(|| repository_error("recovery prepared action 缺少 prepare command"))?;
    let prepare_request_hash = claim
        .prepare_request_hash
        .ok_or_else(|| repository_error("recovery prepared action 缺少 prepare digest"))?;
    let result = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET execution_state = 'enqueued',
               activate_command_id = $1,
               activate_request_digest = $2,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $3
           WHERE id = $4
             AND recovery_id = $5
             AND execution_state = 'prepared'
             AND state_version = $6
             AND claim_token = $7
             AND claim_fencing = $8
             AND delivery_id = $9
             AND prepare_command_id = $10
             AND prepare_request_digest = $11"#,
    )
    .bind(activate_command_id.as_uuid())
    .bind(activate_request_hash.as_bytes().to_vec())
    .bind(activated_at)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(claim.state_version)
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .bind(delivery_id)
    .bind(prepare_command_id.as_uuid())
    .bind(prepare_request_hash.as_bytes().to_vec())
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("recovery action activation fence 已失效"));
    }
    let state_version = claim
        .state_version
        .checked_add(1)
        .ok_or_else(|| repository_error("recovery action 状态版本溢出"))?;
    Ok(RecoveryActionClaim {
        action_id: claim.action_id,
        recovery_id: claim.recovery_id,
        kind: claim.kind.clone(),
        world_id: claim.world_id.clone(),
        buyer_user_id: claim.buyer_user_id,
        recipient: claim.recipient.clone(),
        recipient_eligible: claim.recipient_eligible,
        recipient_state_version: claim.recipient_state_version,
        payment_reference: claim.payment_reference.clone(),
        source_external_mail_id: claim.source_external_mail_id.clone(),
        expected_credits: claim.expected_credits,
        expected_cod: claim.expected_cod,
        expected_attachment_ids: claim.expected_attachment_ids.clone(),
        execution_state: "enqueued".to_owned(),
        delivery_id: Some(delivery_id),
        prepare_command_id: Some(prepare_command_id),
        prepare_request_hash: Some(prepare_request_hash),
        activate_command_id: Some(activate_command_id),
        activate_request_hash: Some(activate_request_hash),
        state_version,
        claim_token: claim.claim_token,
        lease_until: claim.lease_until,
        fencing_version: claim.fencing_version,
        retry_count: claim.retry_count,
        worker_id: claim.worker_id.clone(),
        claimed_at: claim.claimed_at,
        created_at: claim.created_at,
    })
}

#[cfg(feature = "postgres")]
async fn mark_action_satisfied_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    proof_kind: &str,
    proof_reference: &str,
    observed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let digest = recovery_proof_digest(claim.action_id.as_uuid(), proof_reference);
    let result = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET execution_state = 'satisfied',
               proof_kind = $1,
               proof_reference = $2,
               proof_digest = $3,
               proof_observed_at = $4,
               completed_at = $4,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $4
           WHERE id = $5
             AND recovery_id = $6
             AND kind = 'buyer_blacklist'
             AND execution_state = 'pending'
             AND state_version = $7
             AND claim_token = $8
             AND claim_fencing = $9"#,
    )
    .bind(proof_kind)
    .bind(proof_reference)
    .bind(digest.to_vec())
    .bind(observed_at)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(claim.state_version)
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("recovery action 非邮件完成 fence 已失效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn observe_action_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    observed_at: DateTime<Utc>,
) -> Result<RecoveryActionOutcome, auction_application::RepositoryError> {
    let delivery_id =
        claim.delivery_id.ok_or_else(|| repository_error("recovery action 缺少 delivery 标识"))?;
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let row = sqlx::query(
        "SELECT status, external_reference, failure_code, result_metadata
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("recovery action delivery 不存在"))?;
    let status: String = row.try_get("status").map_err(map_storage_error)?;
    let external_reference: Option<String> =
        row.try_get("external_reference").map_err(map_storage_error)?;
    let failure_code: Option<String> = row.try_get("failure_code").map_err(map_storage_error)?;
    let result_metadata: serde_json::Value =
        row.try_get("result_metadata").map_err(map_storage_error)?;

    let (outcome, execution_state, terminal_error, proof_kind, proof_reference, proof_digest) =
        match status.as_str() {
            "prepared" | "pending" | "claimed" | "sending" => {
                transaction.commit().await.map_err(map_storage_error)?;
                return Ok(RecoveryActionOutcome::Retry);
            }
            "succeeded" => {
                let proven = match claim.kind.as_str() {
                    "payment_reminder" => {
                        result_metadata
                            .get("external_reference_proven")
                            .and_then(serde_json::Value::as_bool)
                            == Some(true)
                            && result_metadata.get("operation").and_then(serde_json::Value::as_str)
                                == Some("send")
                            && result_metadata.get("template").and_then(serde_json::Value::as_str)
                                == Some("recovery_payment_reminder")
                    }
                    "reclaim_equipment_take" | "cancel_carrier_take" | "recover_equipment_take" => {
                        result_metadata.get("operation").and_then(serde_json::Value::as_str)
                            == Some("take")
                            && result_metadata
                                .get("external_mail_id")
                                .and_then(serde_json::Value::as_str)
                                == claim.source_external_mail_id.as_deref()
                            && result_metadata.get("proof").and_then(serde_json::Value::as_str)
                                == Some("action_removed_or_success_page")
                            && result_metadata
                                .get("expected_credits")
                                .and_then(serde_json::Value::as_i64)
                                == Some(claim.expected_credits)
                            && result_metadata
                                .get("expected_cod")
                                .and_then(serde_json::Value::as_i64)
                                == Some(claim.expected_cod)
                            && result_metadata
                                .get("expected_attachment_ids")
                                .and_then(serde_json::Value::as_array)
                                .is_some_and(|values| {
                                    values.len() == claim.expected_attachment_ids.len()
                                        && values.iter().zip(&claim.expected_attachment_ids).all(
                                            |(value, expected)| value.as_str() == Some(expected),
                                        )
                                })
                    }
                    _ => false,
                };
                let reference_matches = match claim.kind.as_str() {
                    "payment_reminder" => {
                        external_reference.as_deref().is_some_and(valid_external_reference)
                    }
                    "reclaim_equipment_take" | "cancel_carrier_take" | "recover_equipment_take" => {
                        claim.source_external_mail_id.as_deref().is_some_and(|source| {
                            let expected_reference =
                                format!("mooglemail:{}:take:{source}", claim.world_id);
                            external_reference.as_deref() == Some(expected_reference.as_str())
                        })
                    }
                    _ => false,
                };
                match external_reference.filter(|value| valid_external_reference(value)) {
                    Some(reference) if proven && reference_matches => {
                        let digest = recovery_proof_digest(claim.action_id.as_uuid(), &reference);
                        let (proof_kind, proof_reference) = if claim.kind == "payment_reminder" {
                            ("delivery_recovery_reminder_sent".to_owned(), reference.clone())
                        } else {
                            ("delivery_recovery_take_completed".to_owned(), reference.clone())
                        };
                        (
                            RecoveryActionOutcome::Satisfied {
                                proof_kind: proof_kind.clone(),
                                proof_reference: proof_reference.clone(),
                            },
                            "satisfied",
                            None,
                            Some(proof_kind),
                            Some(proof_reference),
                            Some(digest.to_vec()),
                        )
                    }
                    _ => (
                        RecoveryActionOutcome::ManualReconcile {
                            failure_code: if proven && !reference_matches {
                                "delivery_reference_mismatch".to_owned()
                            } else if proven {
                                "delivery_reference_missing".to_owned()
                            } else {
                                "delivery_reference_unproven".to_owned()
                            },
                        },
                        "manual_reconcile",
                        Some(if proven && !reference_matches {
                            "delivery_reference_mismatch".to_owned()
                        } else if proven {
                            "delivery_reference_missing".to_owned()
                        } else {
                            "delivery_reference_unproven".to_owned()
                        }),
                        None,
                        None,
                        None,
                    ),
                }
            }
            "failed" => {
                let code = failure_code
                    .filter(|value| valid_failure_code(value))
                    .unwrap_or_else(|| "delivery_failed".to_owned());
                (
                    RecoveryActionOutcome::Failed { failure_code: code.clone() },
                    "failed",
                    Some(code),
                    None,
                    None,
                    None,
                )
            }
            "manual_reconcile" => {
                let code = failure_code
                    .filter(|value| valid_failure_code(value))
                    .unwrap_or_else(|| "delivery_outcome_unknown".to_owned());
                (
                    RecoveryActionOutcome::ManualReconcile { failure_code: code.clone() },
                    "manual_reconcile",
                    Some(code),
                    None,
                    None,
                    None,
                )
            }
            _ => {
                transaction.commit().await.map_err(map_storage_error)?;
                return Err(repository_error("recovery action delivery 状态未知"));
            }
        };

    let updated = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET execution_state = $1,
               last_error_code = $2,
               proof_kind = $3,
               proof_reference = $4,
               proof_digest = $5,
               proof_observed_at = $6,
               completed_at = $6,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $6
           WHERE id = $7
             AND recovery_id = $8
             AND execution_state = 'enqueued'
             AND state_version = $9
             AND claim_token = $10
             AND claim_fencing = $11"#,
    )
    .bind(execution_state)
    .bind(terminal_error)
    .bind(proof_kind)
    .bind(proof_reference)
    .bind(proof_digest)
    .bind(observed_at)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(claim.state_version)
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error("recovery action 结果 fence 已失效"));
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(outcome)
}

#[cfg(feature = "postgres")]
async fn release_action_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    let result = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = $1,
               last_error_code = $2,
               state_version = state_version + 1,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $3
             AND recovery_id = $4
             AND execution_state = $5
             AND state_version = $6
             AND claim_token = $7
             AND claim_fencing = $8"#,
    )
    .bind(retry_at)
    .bind(failure_code)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(&claim.execution_state)
    .bind(claim.state_version)
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("recovery action 释放 fence 已失效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn manual_action_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    let result = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET execution_state = 'manual_reconcile',
               last_error_code = $1,
               completed_at = CURRENT_TIMESTAMP,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $2
             AND recovery_id = $3
             AND execution_state = $4
             AND state_version = $5
             AND claim_token = $6
             AND claim_fencing = $7"#,
    )
    .bind(failure_code)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(&claim.execution_state)
    .bind(claim.state_version)
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("recovery action 人工复核 fence 已失效"));
    }
    Ok(())
}

fn valid_failure_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

#[cfg(feature = "postgres")]
fn request_hash_from_optional(
    value: Option<Vec<u8>>,
) -> Result<Option<RequestHash>, auction_application::RepositoryError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let bytes: [u8; 32] =
        value.try_into().map_err(|_| repository_error("recovery action 请求摘要长度无效"))?;
    if bytes == [0; 32] {
        return Err(repository_error("recovery action 请求摘要不能为零"));
    }
    Ok(Some(RequestHash::new(bytes)))
}

#[cfg(feature = "postgres")]
fn valid_external_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

#[cfg(feature = "postgres")]
fn valid_recovery_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

#[cfg(feature = "postgres")]
fn recovery_proof_digest(action_id: Uuid, reference: &str) -> [u8; 32] {
    let digest = Sha256::digest(format!("recovery-action:{action_id}:{reference}").as_bytes());
    let mut result = [0_u8; 32];
    result.copy_from_slice(&digest);
    result
}

#[cfg(feature = "postgres")]
fn map_due_recovery_row(
    row: sqlx::postgres::PgRow,
) -> Result<DueRecoveryRow, auction_application::RepositoryError> {
    Ok(DueRecoveryRow {
        id: row.try_get("id").map_err(map_storage_error)?,
        stage: row.try_get("stage").map_err(map_storage_error)?,
        state_version: row.try_get("state_version").map_err(map_storage_error)?,
        reminder_count: row.try_get("reminder_count").map_err(map_storage_error)?,
        reminder_limit: row.try_get("reminder_limit").map_err(map_storage_error)?,
        world_id: row.try_get("world_id").map_err(map_storage_error)?,
        payment_execution_state: row
            .try_get("payment_execution_state")
            .map_err(map_storage_error)?,
        payment_completed_at: row.try_get("payment_completed_at").map_err(map_storage_error)?,
        equipment_execution_state: row
            .try_get("equipment_execution_state")
            .map_err(map_storage_error)?,
        equipment_completed_at: row.try_get("equipment_completed_at").map_err(map_storage_error)?,
        created_at: row.try_get("created_at").map_err(map_storage_error)?,
    })
}

#[cfg(feature = "postgres")]
fn map_storage_error(error: sqlx::Error) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(match error {
        sqlx::Error::Database(database) if database.code().as_deref() == Some("40001") => {
            "PostgreSQL recovery Worker 发生序列化冲突".to_owned()
        }
        _ => "PostgreSQL recovery case 推进失败".to_owned(),
    })
}
