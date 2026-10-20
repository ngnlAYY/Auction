//! 未付款 COD 被退回后的重发与三次退回收口。
//!
//! 第 1、2 次退回：把原 COD 投递重置为 pending 并重新排队履约，沿用原金额与装备；
//! 第 3 次退回：为卖家建立 withdrawn_return 义务把装备寄回，并把原 COD 与卖家收款按失败收口。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(in crate::database::recovery_commands) async fn advance_cod_return_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    let mut transaction = pool.begin().await.map_err(map_sql_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(map_sql_error)?;

    let Some(candidate) = sqlx::query(
        r#"SELECT recovery.id,
                  recovery.auction_id,
                  recovery.lot_id,
                  recovery.equipment_id,
                  recovery.payment_obligation_id,
                  recovery.seller_payout_obligation_id,
                  recovery.cod_return_count,
                  recovery.cod_return_limit,
                  recovery.state_version
           FROM public.settlement_recoveries AS recovery
           WHERE recovery.stage = 'reclaiming'
             AND recovery.world_id = 'main'
             AND recovery.blocking_code IS NULL
             AND recovery.custody_proof_kind IS NOT NULL
             AND recovery.cod_return_count BETWEEN 1 AND recovery.cod_return_limit
             AND EXISTS (
                 SELECT 1
                 FROM public.settlement_recovery_actions AS reclaim
                 WHERE reclaim.recovery_id = recovery.id
                   AND reclaim.kind = 'reclaim_equipment_take'
                   AND reclaim.execution_state = 'satisfied'
             )
             AND (
                 recovery.cod_return_count < recovery.cod_return_limit
                 OR EXISTS (
                     SELECT 1
                     FROM public.settlement_recovery_actions AS blacklist
                     WHERE blacklist.recovery_id = recovery.id
                       AND blacklist.kind = 'buyer_blacklist'
                       AND blacklist.execution_state = 'satisfied'
                 )
             )
           ORDER BY recovery.updated_at, recovery.id
           LIMIT 1
           FOR UPDATE OF recovery SKIP LOCKED"#,
    )
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_sql_error)?
    else {
        transaction.commit().await.map_err(map_sql_error)?;
        return Ok(None);
    };

    let recovery_id: Uuid = candidate.try_get("id").map_err(map_sql_error)?;
    let state_version: i64 = candidate.try_get("state_version").map_err(map_sql_error)?;
    let payment_obligation_id: Uuid =
        candidate.try_get("payment_obligation_id").map_err(map_sql_error)?;
    let seller_payout_obligation_id: Uuid =
        candidate.try_get("seller_payout_obligation_id").map_err(map_sql_error)?;
    let cod_return_count: i32 = candidate.try_get("cod_return_count").map_err(map_sql_error)?;
    let cod_return_limit: i32 = candidate.try_get("cod_return_limit").map_err(map_sql_error)?;
    if !(1..=10).contains(&cod_return_limit) || cod_return_count > cod_return_limit {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    let auction_id: Uuid = candidate.try_get("auction_id").map_err(map_sql_error)?;
    let lot_id: Uuid = candidate.try_get("lot_id").map_err(map_sql_error)?;
    let equipment_id: Uuid = candidate.try_get("equipment_id").map_err(map_sql_error)?;

    let (stage, resolution, next_state_version) = if cod_return_count < cod_return_limit {
        resend_cod(&mut transaction, now, recovery_id, payment_obligation_id, state_version).await?
    } else {
        return_to_seller(
            &mut transaction,
            now,
            SellerReturnContext {
                recovery_id,
                state_version,
                payment_obligation_id,
                seller_payout_obligation_id,
                auction_id,
                lot_id,
                equipment_id,
            },
        )
        .await?
    };

    transaction.commit().await.map_err(map_sql_error)?;
    Ok(Some(RecoveryResolutionAdvance {
        recovery_id: recovery_id.into(),
        stage: stage.clone(),
        resolution,
        relisted_auction_id: None,
        relisted_lot_id: None,
        state_version: next_state_version,
        completed: stage == "completed",
        advanced_at: now,
    }))
}

/// 未达冻结上限的退回：重置原 COD 投递与履约；追加消费事实保留，原退件不会再次计数。
#[cfg(feature = "postgres")]
async fn resend_cod(
    transaction: &mut Transaction<'_, Postgres>,
    now: DateTime<Utc>,
    recovery_id: Uuid,
    payment_obligation_id: Uuid,
    state_version: i64,
) -> Result<(String, Option<String>, i64), RecoveryCommandError> {
    let delivery = sqlx::query_scalar::<_, Uuid>(
        r#"UPDATE public.deliveries AS delivery
           SET status = 'pending',
               external_reference = NULL,
               failure_code = NULL,
               completed_at = NULL,
               claimed_at = NULL,
               sending_at = NULL,
               lease_token = NULL,
               lease_expires_at = NULL,
               attempt_count = 0,
               fencing_version = delivery.fencing_version + 1,
               result_metadata = '{}'::jsonb,
               updated_at = $2
           FROM public.settlement_obligation_fulfillments AS fulfillment
           WHERE fulfillment.obligation_id = $1
             AND delivery.id = fulfillment.delivery_id
             -- 退回邮件已存在说明确实寄出过；发送证明可能停在人工核对，一并重置。
             AND delivery.status IN ('succeeded', 'manual_reconcile')
           RETURNING delivery.id"#,
    )
    .bind(payment_obligation_id)
    .bind(now)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::InvalidStoredResult)?;
    let _ = delivery;

    let fulfillment = sqlx::query_scalar::<_, Uuid>(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'enqueued',
               state_version = state_version + 1,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               retry_count = 0,
               next_attempt_at = $2,
               last_error_code = NULL,
               proof_kind = NULL,
               proof_reference = NULL,
               proof_digest = NULL,
               proof_observed_at = NULL,
               completed_at = NULL,
               updated_at = $2
           WHERE obligation_id = $1
             AND execution_state NOT IN ('satisfied', 'failed')
           RETURNING obligation_id"#,
    )
    .bind(payment_obligation_id)
    .bind(now)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::InvalidStoredResult)?;
    let _ = fulfillment;

    let next_state_version: i64 = sqlx::query_scalar(
        r#"UPDATE public.settlement_recoveries
           SET stage = 'awaiting_payment',
               payment_deadline_at = $2 + (payment_deadline_at - created_at),
               reminder_count = 0,
               return_mail_id = NULL,
               return_external_mail_id = NULL,
               custody_proof_kind = NULL,
               custody_proof_reference = NULL,
               custody_proof_digest = NULL,
               custody_proof_observed_at = NULL,
               blocking_code = NULL,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $2
           WHERE id = $1
             AND stage = 'reclaiming'
             AND state_version = $3
           RETURNING state_version"#,
    )
    .bind(recovery_id)
    .bind(now)
    .bind(state_version)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;

    Ok(("awaiting_payment".to_owned(), None, next_state_version))
}

/// 第 3 次退回需要一次性锁定的恢复事实。
#[cfg(feature = "postgres")]
struct SellerReturnContext {
    recovery_id: Uuid,
    state_version: i64,
    payment_obligation_id: Uuid,
    seller_payout_obligation_id: Uuid,
    auction_id: Uuid,
    lot_id: Uuid,
    equipment_id: Uuid,
}

/// 第 3 次退回：装备寄回卖家；原 COD 与卖家收款义务按失败收口并记录理由。
#[cfg(feature = "postgres")]
async fn return_to_seller(
    transaction: &mut Transaction<'_, Postgres>,
    now: DateTime<Utc>,
    context: SellerReturnContext,
) -> Result<(String, Option<String>, i64), RecoveryCommandError> {
    let SellerReturnContext {
        recovery_id,
        state_version,
        payment_obligation_id,
        seller_payout_obligation_id,
        auction_id,
        lot_id,
        equipment_id,
    } = context;
    let obligation_id = legacy_natural_key_uuid(
        "auction.recovery.cod-return.seller-return.v1",
        &[recovery_id.as_bytes()],
    );
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO public.settlement_obligations (
               id, auction_id, lot_id, world_id, origin_command_id, policy_version,
               obligation_kind, business_state, recipient_game_identity, amount,
               carrier_credit, equipment_id, submission_id, created_at, updated_at)
           SELECT $1, recovery.auction_id, recovery.lot_id, recovery.world_id,
                  payout.origin_command_id, payout.policy_version,
                  'withdrawn_return', 'ready', payout.recipient_game_identity, 0, 0,
                  recovery.equipment_id, lot.submission_id, $2, $2
           FROM public.settlement_recoveries AS recovery
           INNER JOIN public.settlement_obligations AS payout
               ON payout.id = recovery.seller_payout_obligation_id
           INNER JOIN public.lots AS lot
               ON lot.auction_id = recovery.auction_id AND lot.id = recovery.lot_id
           WHERE recovery.id = $3
             AND recovery.auction_id = $4
             AND recovery.lot_id = $5
             AND recovery.equipment_id = $6
           ON CONFLICT (auction_id, lot_id, obligation_kind) DO NOTHING
           RETURNING id"#,
    )
    .bind(obligation_id)
    .bind(now)
    .bind(recovery_id)
    .bind(auction_id)
    .bind(lot_id)
    .bind(equipment_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    // 义务已存在说明上一轮已完成收口；保持幂等。
    let _ = inserted;

    for (obligation, failure_code) in [
        (payment_obligation_id, "cod_returned_three_times"),
        (seller_payout_obligation_id, "equipment_returned_to_seller"),
    ] {
        let _updated = sqlx::query_scalar::<_, Uuid>(
            r#"UPDATE public.settlement_obligation_fulfillments
               SET execution_state = 'failed',
                   state_version = state_version + 1,
                   claim_token = NULL,
                   claimed_at = NULL,
                   claim_expires_at = NULL,
                   next_attempt_at = NULL,
                   last_error_code = $2,
                   proof_kind = NULL,
                   proof_reference = NULL,
                   proof_digest = NULL,
                   proof_observed_at = NULL,
                   completed_at = $3,
                   updated_at = $3
               WHERE obligation_id = $1
                 AND execution_state NOT IN ('failed', 'satisfied')
               RETURNING obligation_id"#,
        )
        .bind(obligation)
        .bind(failure_code)
        .bind(now)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }

    let next_state_version: i64 = sqlx::query_scalar(
        r#"UPDATE public.settlement_recoveries
           SET stage = 'completed',
               resolution = 'returned_after_three_cod_returns',
               completed_at = $2,
               return_mail_id = NULL,
               return_external_mail_id = NULL,
               blocking_code = NULL,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $2
           WHERE id = $1
             AND stage = 'reclaiming'
             AND state_version = $3
           RETURNING state_version"#,
    )
    .bind(recovery_id)
    .bind(now)
    .bind(state_version)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;

    Ok((
        "completed".to_owned(),
        Some("returned_after_three_cod_returns".to_owned()),
        next_state_version,
    ))
}
