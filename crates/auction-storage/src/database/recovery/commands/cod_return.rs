//! 未付款 COD 被退回后的重发与三次退回收口。
//!
//! 第 1、2 次退回：把原 COD 投递重置为 pending 并重新排队履约，沿用原金额与装备；
//! 第 3 次退回：为卖家建立 withdrawn_return 义务把装备寄回，并把原 COD 与卖家收款按失败收口；
//! 成交随之撤销，同事务冲正补偿池。

use crate::database::delivery::audit::{
    COD_RETURN_RESEND, DeliveryAudit, append as append_delivery_audit,
};
use {
    crate::database::TrackedTransaction as Transaction,
    crate::database::recovery::commands::{is_conflict, map_reversal_error, map_sql_error},
    auction_application::recovery::{RecoveryCommandError, RecoveryResolutionAdvance},
    auction_domain::legacy_natural_key_uuid,
    chrono::{DateTime, Utc},
    serde_json::Value,
    sqlx::{Postgres, Row},
    uuid::Uuid,
};

pub(in crate::database::recovery::commands) async fn advance_cod_return_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    // 退回卖家时要锁补偿池，可能与任一世界的结算发生序列化冲突；冲突有界重试。
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Recovery,
        crate::database::SERIALIZABLE_ATTEMPTS,
        is_conflict,
        || advance_cod_return_postgres_once(pool, now),
    )
    .await
}

async fn advance_cod_return_postgres_once(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    let mut transaction =
        pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(map_sql_error)?;

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
async fn resend_cod(
    transaction: &mut Transaction<'_, Postgres>,
    now: DateTime<Utc>,
    recovery_id: Uuid,
    payment_obligation_id: Uuid,
    state_version: i64,
) -> Result<(String, Option<String>, i64), RecoveryCommandError> {
    // 清空当前结果之前冻结安全投影；退件证明只授权本次回收重排，不补造旧 Sent 证明。
    let previous = sqlx::query(
        r#"SELECT delivery.id, delivery.command_id, delivery.fencing_version,
                  jsonb_build_object(
                      'status', 'pending',
                      'original_delivery_id', delivery.id,
                      'original_command_id', delivery.command_id,
                      'previous_status', delivery.status,
                      'previous_fencing_version', delivery.fencing_version,
                      'fencing_version', delivery.fencing_version + 1,
                      'previous_attempt_count', delivery.attempt_count,
                      'previous_claimed_at', delivery.claimed_at,
                      'previous_sending_at', delivery.sending_at,
                      'previous_completed_at', delivery.completed_at,
                      'recovery_id', recovery.id,
                      'payment_obligation_id', recovery.payment_obligation_id,
                      'return_count', recovery.cod_return_count,
                      'return_limit', recovery.cod_return_limit,
                      'return_mail_id', recovery.return_mail_id,
                      'return_external_mail_id', recovery.return_external_mail_id,
                      'return_consumed_at', consumed.recorded_at,
                      'reclaim_action_id', reclaim.id,
                      'reclaim_delivery_id', reclaim.delivery_id,
                      'custody_proof_kind', recovery.custody_proof_kind,
                      'custody_proof_reference', recovery.custody_proof_reference,
                      'custody_proof_digest', encode(recovery.custody_proof_digest, 'hex'),
                      'custody_proof_observed_at', recovery.custody_proof_observed_at,
                      'proven_sent', CASE
                          WHEN delivery.status = 'succeeded'
                           AND delivery.failure_code IS NULL
                           AND delivery.sending_at IS NOT NULL
                           AND delivery.completed_at >= delivery.sending_at
                           AND delivery.result_metadata->>'operation' = 'send'
                           AND delivery.result_metadata->'external_reference_proven' = 'true'::jsonb
                           AND delivery.external_reference ~ '^mooglemail:main:send:[0-9]{1,32}$'
                           AND (delivery.result_metadata->>'proof' = 'sent_recovered_reference_v1'
                                OR (delivery.result_metadata->>'proof' = 'sent_exact_payload_v1'
                                    AND delivery.result_metadata->'sent_verified' = 'true'::jsonb)
                                OR public.delivery_has_frozen_sent_proof(
                                    delivery.id, delivery.result_metadata, delivery.external_reference))
                          THEN jsonb_build_object(
                              'external_reference', delivery.external_reference,
                              'proof', delivery.result_metadata->>'proof')
                          ELSE NULL
                      END
                  ) AS audit_metadata
           FROM public.settlement_obligation_fulfillments AS fulfillment
           JOIN public.deliveries AS delivery ON delivery.id = fulfillment.delivery_id
           JOIN public.settlement_recoveries AS recovery
               ON recovery.id = $2 AND recovery.payment_obligation_id = fulfillment.obligation_id
           JOIN public.settlement_recovery_return_consumptions AS consumed
               ON consumed.recovery_id = recovery.id
              AND consumed.return_mail_id = recovery.return_mail_id
           JOIN public.inbound_mail AS inbound
               ON inbound.id = recovery.return_mail_id
              AND inbound.external_mail_id = recovery.return_external_mail_id
              AND inbound.world_id = 'main'
           JOIN public.settlement_recovery_actions AS reclaim
               ON reclaim.recovery_id = recovery.id
              AND reclaim.kind = 'reclaim_equipment_take'
              AND reclaim.execution_state = 'satisfied'
              AND reclaim.source_mail_id = recovery.return_mail_id
              AND reclaim.proof_kind = 'delivery_recovery_take_succeeded'
              AND reclaim.proof_kind = recovery.custody_proof_kind
              AND reclaim.proof_reference = recovery.return_external_mail_id
              AND reclaim.proof_reference = recovery.custody_proof_reference
              AND reclaim.proof_digest = recovery.custody_proof_digest
           WHERE fulfillment.obligation_id = $1
             AND delivery.origin_kind = 'settlement'
             AND delivery.origin_id = fulfillment.obligation_id::text
             AND delivery.status IN ('succeeded', 'manual_reconcile')
             AND recovery.stage = 'reclaiming' AND recovery.state_version = $3
             AND recovery.cod_return_count BETWEEN 1 AND recovery.cod_return_limit - 1
             AND recovery.return_external_mail_id ~ '^[0-9]{1,32}$'
           FOR UPDATE OF delivery"#,
    )
    .bind(payment_obligation_id)
    .bind(recovery_id)
    .bind(state_version)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::InvalidStoredResult)?;
    let delivery_id: Uuid = previous.try_get("id").map_err(map_sql_error)?;
    let command_id: Uuid = previous.try_get("command_id").map_err(map_sql_error)?;
    let previous_fencing: i64 = previous.try_get("fencing_version").map_err(map_sql_error)?;
    let audit_metadata: Value = previous.try_get("audit_metadata").map_err(map_sql_error)?;
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
           WHERE delivery.id = $1
             AND delivery.fencing_version = $3
             AND delivery.status IN ('succeeded', 'manual_reconcile')
           RETURNING delivery.id"#,
    )
    .bind(delivery_id)
    .bind(now)
    .bind(previous_fencing)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::InvalidStoredResult)?;
    if delivery != delivery_id {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    let request_id = command_id.to_string();
    append_delivery_audit(
        transaction,
        DeliveryAudit {
            occurred_at: now,
            actor_type: "system",
            actor_id: "recovery-service",
            action: COD_RETURN_RESEND,
            delivery_id,
            request_id: Some(&request_id),
            source: "system",
            metadata: audit_metadata,
        },
    )
    .await
    .map_err(map_sql_error)?;

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

    // 按 recovery 冻结的版本重新起算付款窗口：v1 七天，v2（赛季前场次）24 小时；
    // 已延长的绝对截止不再参与计算。
    let next_state_version: i64 = sqlx::query_scalar(
        r#"UPDATE public.settlement_recoveries
           SET stage = 'awaiting_payment',
               payment_deadline_at = $2 + CASE policy_version
                   WHEN 2 THEN interval '86400 seconds'
                   ELSE interval '604800 seconds'
               END,
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
             AND policy_version IN (1, 2)
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
    // 收口会冲正补偿池；先锁池，再写入经外键引用 worlds 的义务，与结算锁顺序一致。
    crate::database::settlement::compensation_reversal::lock_pool(transaction)
        .await
        .map_err(map_reversal_error)?;
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

    // COD 三次退回以统一处置事实收口；读取侧据此解除两个失败义务的告警。
    // 处置身份仍绑定原 COD 投递，来源引用恢复记录；缺失投递时保留告警，不伪造事实。
    let payment_delivery: Option<(Uuid, i64)> = sqlx::query_as(
        r#"SELECT fulfillment.delivery_id, delivery.fencing_version
           FROM public.settlement_obligation_fulfillments AS fulfillment
           INNER JOIN public.deliveries AS delivery ON delivery.id = fulfillment.delivery_id
           WHERE fulfillment.obligation_id = $1
             AND fulfillment.delivery_id IS NOT NULL"#,
    )
    .bind(payment_obligation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if let Some((delivery_id, fencing_version)) = payment_delivery {
        crate::database::delivery::disposition::append(
            &mut *transaction,
            crate::database::delivery::disposition::DeliveryDispositionFact {
                delivery_id,
                fencing_version,
                kind: auction_domain::DeliveryDispositionKind::CodReturned,
                source: auction_domain::DeliveryDispositionSource {
                    settlement_recovery_id: Some(recovery_id),
                    ..Default::default()
                },
                actor_source: auction_domain::DeliveryDispositionActorSource::Worker,
                actor_id: "recovery-cod-return",
                reason: Some("COD returned to seller after three attempts"),
                reason_code: Some("cod_returned_three_times"),
                detail: Some(&serde_json::json!({
                    "recovery_id": recovery_id,
                    "payment_obligation_id": payment_obligation_id,
                    "seller_payout_obligation_id": seller_payout_obligation_id,
                })),
                command_id: None,
                request_digest: None,
            },
        )
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
    // 买方始终未付款且装备已退回卖家，成交撤销；补偿池冲正与 resolution 落定同事务提交。
    crate::database::settlement::compensation_reversal::reverse_voided_sale(
        transaction,
        recovery_id,
        now,
    )
    .await
    .map_err(map_reversal_error)?;

    Ok((
        "completed".to_owned(),
        Some("returned_after_three_cod_returns".to_owned()),
        next_state_version,
    ))
}
