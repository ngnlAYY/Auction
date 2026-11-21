//! 达到冻结退回上限的 COD 退回：装备寄回卖家并收口原成交；被恢复接管而停在待发的原 COD 投递
//! 同事务终结。

use auction_application::recovery::RecoveryCommandError;
use auction_domain::legacy_natural_key_uuid;
use auction_time::Timestamp;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::recovery::commands::mapping::{map_reversal_error, map_sql_error};
use crate::database::recovery::commands::superseded::{
    SupersedingActor, retire_superseded_dispatch,
};

/// 退回达到冻结上限时需要一次性锁定的恢复事实。
pub(super) struct SellerReturnContext {
    pub(super) recovery_id: Uuid,
    pub(super) state_version: i64,
    pub(super) payment_obligation_id: Uuid,
    pub(super) seller_payout_obligation_id: Uuid,
    pub(super) auction_id: Uuid,
    pub(super) lot_id: Uuid,
    pub(super) equipment_id: Uuid,
    pub(super) cod_return_count: i32,
    pub(super) cod_return_limit: i32,
}

/// 退回达到冻结上限：装备寄回卖家；原 COD 与卖家收款义务按失败收口并记录理由。
pub(super) async fn return_to_seller(
    transaction: &mut Transaction<'_, Postgres>,
    now: Timestamp,
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
        cod_return_count,
        cod_return_limit,
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

    // 原 COD 投递在退件到达前被重新排队、因恢复接管而停在待发（当前 fence 从未进入发送围栏）时，
    // 同事务终结为 `failed/recovery_superseded`；随后的 `cod_returned` 处置落在终结后的 fence 上。
    retire_superseded_dispatch(
        transaction,
        payment_obligation_id,
        recovery_id,
        &SupersedingActor { actor_type: "system", actor_id: "recovery-service", source: "system" },
        now,
    )
    .await?;

    for (obligation, failure_code) in [
        (payment_obligation_id, "cod_returned_three_times"),
        (seller_payout_obligation_id, "equipment_returned_to_seller"),
    ] {
        let updated = sqlx::query_scalar::<_, Uuid>(
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
        if updated.is_none() {
            let already_failed: bool = sqlx::query_scalar(
                r#"SELECT EXISTS(
                       SELECT 1
                       FROM public.settlement_obligation_fulfillments
                       WHERE obligation_id = $1
                         AND execution_state = 'failed'
                         AND last_error_code = $2
                   )"#,
            )
            .bind(obligation)
            .bind(failure_code)
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            if !already_failed {
                return Err(RecoveryCommandError::StaleState);
            }
        }
    }

    // COD 退回达到冻结上限后以统一处置事实收口；读取侧据此解除两个失败义务的告警。
    // 处置身份仍绑定原 COD 投递，来源引用恢复记录；缺失投递时保留告警，不伪造事实。
    // 稳定码沿用 cod_returned_three_times，理由写实际次数与冻结上限；已追加的历史事实不改写。
    let reason = format!(
        "COD returned to seller after {cod_return_count} returns (limit {cod_return_limit})"
    );
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
                reason: Some(&reason),
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
