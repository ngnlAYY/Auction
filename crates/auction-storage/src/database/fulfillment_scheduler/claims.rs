//! 结算履约 due claim 与 claim payload 映射。

use super::mapping::*;

use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn claim_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<SettlementFulfillmentClaim>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let rows = sqlx::query(
        r#"SELECT f.obligation_id, f.execution_state, f.state_version,
                  f.claim_fencing, f.retry_count, f.dispatch_world,
                  f.delivery_operation,
                  f.delivery_id, f.prepare_command_id, f.prepare_request_digest,
                  delivery.external_reference AS delivery_external_reference,
                  delivery.result_metadata ->> 'external_reference_proven'
                      AS delivery_external_reference_proven,
                  o.auction_id, o.lot_id, o.origin_command_id, o.obligation_kind,
                  o.recipient_user_id, recipient_user.game_player_id AS recipient_user_game_identity,
                  o.recipient_game_identity, o.amount, o.carrier_credit, o.equipment_id,
                  equipment.external_equipment_id, equipment.name AS equipment_name,
                  o.submission_id, submission.source_kind AS submission_source_kind,
                  submission.external_mail_id AS submission_external_mail_id,
                  o.prerequisite_obligation_id
           FROM public.settlement_obligation_fulfillments AS f
           INNER JOIN public.settlement_obligations AS o
               ON o.id = f.obligation_id
           INNER JOIN public.equipment AS equipment
               ON equipment.id = o.equipment_id
           LEFT JOIN public.users AS recipient_user
               ON recipient_user.id = o.recipient_user_id
           LEFT JOIN public.auction_submissions AS submission
               ON submission.id = o.submission_id
           LEFT JOIN public.settlement_obligation_fulfillments AS prerequisite
               ON prerequisite.obligation_id = o.prerequisite_obligation_id
           LEFT JOIN public.deliveries AS delivery
               ON delivery.id = f.delivery_id
           WHERE f.execution_state IN ('pending', 'prepared', 'enqueued')
             AND (f.execution_state <> 'enqueued' OR delivery.status = 'succeeded')
             AND (f.claim_token IS NULL OR f.claim_expires_at <= $1)
             AND (f.next_attempt_at IS NULL OR f.next_attempt_at <= $1)
             AND o.business_state = 'ready'
             AND (
                 o.prerequisite_obligation_id IS NULL
                 OR prerequisite.execution_state = 'satisfied'
             )
           ORDER BY COALESCE(f.next_attempt_at, f.created_at), f.obligation_id
           LIMIT $2
           FOR UPDATE OF f, o SKIP LOCKED"#,
    )
    .bind(now)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let obligation_id = uuid_column(&row, "obligation_id")?;
        let claim_token = Uuid::new_v4();
        let old_fencing: i64 = row.try_get("claim_fencing").map_err(map_storage_error)?;
        let claim_fencing = old_fencing
            .checked_add(1)
            .ok_or_else(|| repository_error_owned("结算履约 fencing 版本溢出"))?;
        if claim_fencing <= 0 {
            return Err(repository_error_owned("结算履约 fencing 版本无效"));
        }
        let old_attempt_count: i32 = row.try_get("retry_count").map_err(map_storage_error)?;
        let attempt_count = old_attempt_count
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| repository_error_owned("结算履约尝试次数溢出"))?;
        let state_version: i64 = row.try_get("state_version").map_err(map_storage_error)?;
        let next_state_version = state_version
            .checked_add(1)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| repository_error_owned("结算履约状态版本溢出"))?;
        let attempt_count_db = i32::try_from(attempt_count)
            .map_err(|_| repository_error_owned("结算履约尝试次数超出数据库范围"))?;
        let updated = sqlx::query(
            r#"UPDATE public.settlement_obligation_fulfillments
               SET claim_token = $2,
                   claimed_at = $3,
                   claim_expires_at = $4,
                   claim_fencing = $5,
                   retry_count = $6,
                   state_version = state_version + 1,
                   next_attempt_at = NULL,
                   last_error_code = NULL,
                   updated_at = CURRENT_TIMESTAMP
               WHERE obligation_id = $1
                 AND execution_state IN ('pending', 'prepared', 'enqueued')
                 AND (claim_token IS NULL OR claim_expires_at <= $3)"#,
        )
        .bind(obligation_id)
        .bind(claim_token)
        .bind(now)
        .bind(lease_until)
        .bind(claim_fencing)
        .bind(attempt_count_db)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(repository_error_owned("结算履约领取时租约已变化"));
        }

        claims.push(map_claim(
            &row,
            worker_id,
            claim_token,
            claim_fencing,
            next_state_version,
            now,
            lease_until,
            attempt_count,
        )?);
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(claims)
}

#[cfg(feature = "postgres")]
// SQL 行映射的参数分别对应 claim 的租约、版本和尝试次数；拆成共享大结构会越过存储 owner 边界。
#[allow(clippy::too_many_arguments)]
pub(super) fn map_claim(
    row: &PgRow,
    worker_id: &str,
    claim_token: Uuid,
    claim_fencing: i64,
    state_version: u64,
    claimed_at: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    attempt_count: u32,
) -> Result<SettlementFulfillmentClaim, auction_application::RepositoryError> {
    let world = settlement_world(string_column(row, "dispatch_world")?)?;
    let operation = fulfillment_operation(string_column(row, "delivery_operation")?)?;
    let kind = obligation_kind(string_column(row, "obligation_kind")?)?;
    let recipient_user_id: Option<Uuid> =
        row.try_get("recipient_user_id").map_err(map_storage_error)?;
    let recipient_user_game_identity: Option<String> =
        row.try_get("recipient_user_game_identity").map_err(map_storage_error)?;
    let recipient_game_identity: Option<String> =
        row.try_get("recipient_game_identity").map_err(map_storage_error)?;
    let recipient = match (recipient_user_id, recipient_user_game_identity, recipient_game_identity)
    {
        (Some(_), Some(identity), None) if !identity.is_empty() && identity == identity.trim() => {
            identity
        }
        (None, None, Some(identity)) if !identity.is_empty() && identity == identity.trim() => {
            identity
        }
        _ => return Err(repository_error_owned("结算履约收件人身份不完整")),
    };
    let submission_id: Option<Uuid> = row.try_get("submission_id").map_err(map_storage_error)?;
    let prerequisite_id: Option<Uuid> =
        row.try_get("prerequisite_obligation_id").map_err(map_storage_error)?;
    let external_reference: Option<String> =
        row.try_get("delivery_external_reference").map_err(map_storage_error)?;
    let external_reference_proven = match row
        .try_get::<Option<String>, _>("delivery_external_reference_proven")
        .map_err(map_storage_error)?
        .as_deref()
    {
        None => false,
        Some("true") => true,
        Some("false") => false,
        Some(_) => return Err(repository_error_owned("结算履约外部引用证明标记无效")),
    };
    Ok(SettlementFulfillmentClaim {
        obligation_id: SettlementId::from_uuid(uuid_column(row, "obligation_id")?),
        auction_id: AuctionId::from_uuid(uuid_column(row, "auction_id")?),
        lot_id: LotId::from_uuid(uuid_column(row, "lot_id")?),
        origin_command_id: CommandId::from_uuid(uuid_column(row, "origin_command_id")?),
        world,
        kind,
        state: match string_column(row, "execution_state")?.as_str() {
            "pending" => SettlementFulfillmentState::Pending,
            "prepared" => SettlementFulfillmentState::Prepared,
            "enqueued" => SettlementFulfillmentState::Enqueued,
            _ => return Err(repository_error_owned("结算履约阶段无效")),
        },
        operation,
        recipient,
        amount: row.try_get("amount").map_err(map_storage_error)?,
        carrier_credit: u32::try_from(
            row.try_get::<i64, _>("carrier_credit").map_err(map_storage_error)?,
        )
        .map_err(|_| repository_error_owned("结算履约 carrier credit 超出范围"))?,
        equipment_id: EquipmentId::from_uuid(uuid_column(row, "equipment_id")?),
        external_equipment_id: string_column(row, "external_equipment_id")?,
        equipment_name: string_column(row, "equipment_name")?,
        submission_id: submission_id.map(SubmissionId::from_uuid),
        submission_source_kind: row.try_get("submission_source_kind").map_err(map_storage_error)?,
        submission_external_mail_id: row
            .try_get("submission_external_mail_id")
            .map_err(map_storage_error)?,
        prerequisite_id: prerequisite_id.map(SettlementId::from_uuid),
        delivery_id: row.try_get("delivery_id").map_err(map_storage_error)?,
        external_reference,
        external_reference_proven,
        prepare_command_id: row
            .try_get::<Option<Uuid>, _>("prepare_command_id")
            .map_err(map_storage_error)?
            .map(CommandId::from_uuid),
        prepare_request_hash: request_hash_from_optional(
            row.try_get("prepare_request_digest").map_err(map_storage_error)?,
        )?,
        claim_token,
        claim_fencing,
        state_version,
        worker_id: worker_id.to_owned(),
        claimed_at,
        lease_until,
        attempt_count,
    })
}
