#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(crate) async fn record_return_postgres(
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
                  recovery.cod_return_count,
                  recovery.cod_return_limit,
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
                 FROM public.settlement_recovery_return_consumptions AS consumed
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

    if !(1..=10).contains(&row.cod_return_limit) {
        return Err(repository_error("recovery COD 退回上限无效"));
    }
    let next_return_count = row
        .cod_return_count
        .checked_add(1)
        .filter(|count| *count <= row.cod_return_limit)
        .ok_or_else(|| repository_error("recovery COD 退回次数超出上限"))?;
    // 消费唯一事实与计数、后续动作同事务提交；重寄只能清当前指针，不能释放原邮件。
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_consumptions
             (return_mail_id, recovery_id, recorded_at) VALUES ($1, $2, $3)",
    )
    .bind(row.return_mail_id)
    .bind(row.recovery_id)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    let updated = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET return_mail_id = $1,
               return_external_mail_id = $2,
               stage = 'reclaiming',
               blocking_code = $3,
               cod_return_count = $4,
               resolution = NULL,
               relisted_auction_id = NULL,
               relisted_lot_id = NULL,
               custody_proof_kind = NULL,
               custody_proof_reference = NULL,
               custody_proof_digest = NULL,
               custody_proof_observed_at = NULL,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $5
           WHERE id = $6
             AND state_version = $7
             AND return_mail_id IS NULL
             AND stage <> 'completed'
           RETURNING id, stage, state_version"#,
    )
    .bind(row.return_mail_id)
    .bind(&row.return_external_mail_id)
    .bind(&blocking_code)
    .bind(next_return_count)
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
                reason: None,
                now,
                reset_existing: true,
            },
        )
        .await?;
        action_kinds.push(expected.kind.to_owned());

        // 达到该 recovery 冻结的退回上限时拉黑买家，并把实际次数写入公开理由。
        if next_return_count >= row.cod_return_limit
            && matches!(expected.kind, "reclaim_equipment_take" | "cancel_carrier_take")
        {
            let reason = auction_application::recovery_buyer_blacklist_reason(
                u32::try_from(next_return_count)
                    .map_err(|_| repository_error("recovery COD 退回次数无效"))?,
            );
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
                    reason: Some(&reason),
                    now,
                    reset_existing: false,
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
