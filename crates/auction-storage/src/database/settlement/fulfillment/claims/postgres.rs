//! 履约义务的 PostgreSQL 领取：有界候选、锁序与领取 fence；行映射见 `row`。

use auction_application::settlement::SettlementFulfillmentClaim;
use auction_time::Timestamp;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::row::map_claim;
use crate::database::repository_error;
use crate::database::settlement::fulfillment::eligibility::recovery_allows_execution;
use crate::database::settlement::fulfillment::mapping::{
    ClaimMappingError, map_claim_execution_identity, map_storage_error, validate_claim_context,
};
use crate::database::settlement::fulfillment::reconcile::manual_reconcile_in_transaction;

pub(crate) async fn claim_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    worker_id: &str,
    now: Timestamp,
    lease_until: Timestamp,
    limit: u16,
) -> Result<Vec<SettlementFulfillmentClaim>, auction_application::core::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    // 先按到期顺序截取有界候选，再按 UUID 锁义务；未锁到的候选留待下一批，
    // 不扫描并锁住全部 ready。先义务后 execution，与满足事务保持相同锁序。
    // 成交撤销后恢复记录的付款、交付与货款义务不再领取，只有 equipment_recovered 后的买方退款
    // 例外（不变量 2、6）；最终事务再以同一谓词复核。
    let obligation_ids = sqlx::query_scalar::<_, Uuid>(concat!(
        r#"WITH candidates AS MATERIALIZED (
               SELECT o.id
               FROM public.settlement_obligations o
               LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id=o.id
               LEFT JOIN public.settlement_obligation_fulfillments prerequisite
                   ON prerequisite.obligation_id=o.prerequisite_obligation_id
               WHERE o.business_state='ready'
                 AND (f.obligation_id IS NULL OR (
                     f.execution_state IN ('pending','prepared','enqueued')
                     AND ((f.claim_token IS NULL
                           AND (f.next_attempt_at IS NULL OR f.next_attempt_at<=$1))
                          OR f.claim_expires_at<=$1)))
                 AND (o.prerequisite_obligation_id IS NULL
                      OR prerequisite.execution_state='satisfied'
                      OR prerequisite.obligation_id IS NULL)
                 AND "#,
        recovery_allows_execution!(),
        r#"
               ORDER BY o.created_at,o.auction_id,o.lot_id,o.obligation_kind,o.id
               LIMIT $2
           )
           SELECT o.id FROM public.settlement_obligations o
           JOIN candidates ON candidates.id=o.id
           WHERE o.business_state='ready'
           ORDER BY o.id FOR UPDATE OF o SKIP LOCKED"#
    ))
    .bind(now)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    // ready obligation 可能尚未有 execution 行；只为缺失行补齐当前派发语义，
    // 不回写已有 delivery 的冻结操作。候选由锁保护，避免两个 Worker 同时补写同一行。
    sqlx::query(
        r#"WITH candidates AS MATERIALIZED (
               SELECT o.id, o.obligation_kind, o.world_id
               FROM public.settlement_obligations AS o
               LEFT JOIN public.settlement_obligation_fulfillments AS f
                   ON f.obligation_id = o.id
               LEFT JOIN public.settlement_obligation_fulfillments AS prerequisite
                   ON prerequisite.obligation_id = o.prerequisite_obligation_id
               WHERE o.business_state = 'ready'
                 AND o.id=ANY($1)
                 AND f.obligation_id IS NULL
                 AND (
                     o.prerequisite_obligation_id IS NULL
                     OR prerequisite.execution_state = 'satisfied'
                     OR prerequisite.obligation_id IS NULL
                 )
           )
           INSERT INTO public.settlement_obligation_fulfillments
               (obligation_id, execution_state, dispatch_world, delivery_operation)
           SELECT candidates.id,
                  'pending',
                  CASE
                      WHEN candidates.obligation_kind IN (
                          'winner_payment_carrier_cod', 'seller_payout', 'buyer_refund'
                      ) THEN 'main'
                      ELSE candidates.world_id
                  END,
                  'send'
           FROM candidates
           ON CONFLICT (obligation_id) DO NOTHING"#,
    )
    .bind(&obligation_ids)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    // 上下文关系故意用 LEFT JOIN 暴露损坏事实；只有非空的 obligation/fulfillment owner 参与锁定，
    // nullable 关系留给后续校验并在同批次隔离。既避免 outer-join 锁错误，又保持父子锁保护。
    let rows = sqlx::query(concat!(
        r#"SELECT f.obligation_id, f.execution_state, f.state_version,
                  f.claim_fencing, f.retry_count, f.dispatch_world,
                  f.delivery_operation, f.dispatch_no,
                  f.delivery_id AS fulfillment_delivery_id,
                  f.prepare_command_id, f.prepare_request_digest,
                  f.activate_command_id, f.activate_request_digest,
                  delivery.id AS delivery_snapshot_id,
                  delivery.status AS delivery_status,
                  delivery.command_id AS delivery_prepare_command_id,
                  delivery.request_digest AS delivery_prepare_request_digest,
                  delivery.activation_command_id AS delivery_activate_command_id,
                  delivery.activation_request_digest AS delivery_activate_request_digest,
                  delivery.origin_kind AS delivery_origin_kind,
                  delivery.origin_id AS delivery_origin_id,
                  delivery.recipient AS delivery_recipient,
                  delivery.payload AS delivery_payload,
                  delivery.external_reference AS delivery_external_reference,
                  delivery.failure_code AS delivery_failure_code,
                  delivery.completed_at AS delivery_completed_at,
                  CASE WHEN delivery.result_metadata ->> 'proof'
                      IN ('sent_recovered_frozen_v1', 'sent_operator_confirmed_v1')
                    THEN CASE WHEN public.delivery_has_frozen_sent_proof(
                      delivery.id, delivery.result_metadata, delivery.external_reference)
                      THEN 'true' ELSE 'false' END
                    ELSE delivery.result_metadata ->> 'external_reference_proven'
                  END AS delivery_external_reference_proven,
                  delivery.result_metadata ->> 'operation' AS delivery_result_operation,
                  delivery.result_metadata ->> 'proof' AS delivery_result_proof,
                  o.id AS obligation_row_id,
                  o.auction_id, o.lot_id, o.origin_command_id, o.obligation_kind,
                  o.world_id AS obligation_world,
                  o.recipient_user_id, recipient_user.username AS recipient_user_game_identity,
                  o.recipient_game_identity, o.amount, o.carrier_credit, o.equipment_id,
                  equipment.id AS equipment_row_id, equipment.world_id AS equipment_world,
                  equipment.external_equipment_id, equipment.name AS equipment_name,
                  settlement.lot_id AS settlement_lot_id,
                  settlement.end_price, settlement.listed_price, settlement.buyer_amount,
                  settlement.seller_amount, settlement.tax_amount, settlement.compensation_amount,
                  settlement.hold_ppm, settlement.adjustment_ppm,
                  settlement.adjustment_metadata ->> 'policy' AS settlement_policy_mode,
                  recovery.payment_deadline_at AS recovery_payment_deadline_at,
                  recovery.created_at AS recovery_created_at,
                  recovery.policy_version AS recovery_policy_version,
                  o.submission_id, submission.source_kind AS submission_source_kind,
                  submission.source_reference AS submission_source_reference,
                  submission.mailbox_key AS submission_mailbox_key,
                  submission.external_mail_id AS submission_external_mail_id,
                  submission.seller AS submission_seller,
                  o.prerequisite_obligation_id,
                  prerequisite.obligation_id AS prerequisite_row_id
           FROM public.settlement_obligation_fulfillments AS f
           JOIN public.settlement_obligations AS o
               ON o.id = f.obligation_id
           LEFT JOIN public.equipment AS equipment
               ON equipment.id = o.equipment_id
           LEFT JOIN public.lot_settlements AS settlement
               ON settlement.lot_id = o.lot_id AND o.obligation_kind = 'winner_equipment_cod'
           LEFT JOIN public.settlement_recoveries AS recovery
               ON recovery.payment_obligation_id = o.id
           LEFT JOIN public.users AS recipient_user
               ON recipient_user.id = o.recipient_user_id
           LEFT JOIN public.auction_submissions AS submission
               ON submission.id = o.submission_id
           LEFT JOIN public.settlement_obligation_fulfillments AS prerequisite
               ON prerequisite.obligation_id = o.prerequisite_obligation_id
           LEFT JOIN public.deliveries AS delivery
               ON delivery.id = f.delivery_id
           WHERE f.execution_state IN ('pending', 'prepared', 'enqueued')
             AND (
                 (
                     f.claim_token IS NULL
                     AND (f.next_attempt_at IS NULL OR f.next_attempt_at <= $1)
                 )
                 OR f.claim_expires_at <= $1
             )
             AND o.business_state = 'ready'
             AND o.id=ANY($3)
             AND (
                 o.prerequisite_obligation_id IS NULL
                 OR prerequisite.execution_state = 'satisfied'
                 OR prerequisite.obligation_id IS NULL
             )
             AND "#,
        recovery_allows_execution!(),
        r#"
           ORDER BY f.obligation_id
           LIMIT $2
           FOR UPDATE OF f SKIP LOCKED"#
    ))
    .bind(now)
    .bind(i64::from(limit))
    .bind(&obligation_ids)
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let execution_identity = map_claim_execution_identity(&row)?;
        let obligation_id = execution_identity.obligation_id;
        let claim_token = Uuid::new_v4();
        let old_fencing: i64 = row.try_get("claim_fencing").map_err(map_storage_error)?;
        let claim_fencing = old_fencing
            .checked_add(1)
            .ok_or_else(|| repository_error("结算履约 fencing 版本溢出"))?;
        if claim_fencing <= 0 {
            return Err(repository_error("结算履约 fencing 版本无效"));
        }
        let retry_count: i32 = row.try_get("retry_count").map_err(map_storage_error)?;
        let retry_count =
            u32::try_from(retry_count).map_err(|_| repository_error("结算履约重试次数无效"))?;
        let state_version: i64 = row.try_get("state_version").map_err(map_storage_error)?;
        let next_state_version = state_version
            .checked_add(1)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| repository_error("结算履约状态版本溢出"))?;
        let updated = sqlx::query(
            r#"UPDATE public.settlement_obligation_fulfillments
               SET claim_token = $2,
                   claimed_at = $3,
                   claim_expires_at = $4,
                   claim_fencing = $5,
                   state_version = state_version + 1,
                   next_attempt_at = NULL,
                   updated_at = CURRENT_TIMESTAMP
               WHERE obligation_id = $1
                 AND execution_state IN ('pending', 'prepared', 'enqueued')
                 AND (
                     (
                         claim_token IS NULL
                         AND (next_attempt_at IS NULL OR next_attempt_at <= $3)
                     )
                     OR claim_expires_at <= $3
                 )"#,
        )
        .bind(obligation_id)
        .bind(claim_token)
        .bind(now)
        .bind(lease_until)
        .bind(claim_fencing)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(repository_error("结算履约领取时租约已变化"));
        }

        if validate_claim_context(&row).is_err() {
            manual_reconcile_in_transaction(
                &mut transaction,
                execution_identity.obligation_id,
                &execution_identity.execution_state,
                next_state_version,
                claim_token,
                claim_fencing,
                "invalid_fulfillment_fact",
            )
            .await?;
            continue;
        }

        match map_claim(
            &row,
            worker_id,
            claim_token,
            claim_fencing,
            next_state_version,
            now,
            lease_until,
            retry_count,
        ) {
            Ok(claim) => claims.push(claim),
            Err(ClaimMappingError::Execution(error)) => return Err(error),
            Err(ClaimMappingError::Context(_error)) => {
                manual_reconcile_in_transaction(
                    &mut transaction,
                    execution_identity.obligation_id,
                    &execution_identity.execution_state,
                    next_state_version,
                    claim_token,
                    claim_fencing,
                    "invalid_fulfillment_fact",
                )
                .await?;
            }
        }
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(claims)
}
