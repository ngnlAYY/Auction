//! Recovery 退回履约记录与下一步 action 建立。
//!
//! 该文件只承载 recovery Worker 的单一职责，事务入口和外部副作用仍由父模块统一约束。

#[cfg(feature = "postgres")]
use super::*;

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
pub(super) async fn record_return_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
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
