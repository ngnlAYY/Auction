//! 冲突 SQL 门：在当前事务内读取同 EID/world 的义务、恢复动作与出站投递，任一未闭合即拒绝。

use std::sync::LazyLock;

use auction_application::settlement::FundingError;
use futures_util::TryStreamExt;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::AssetConflictTarget;
use super::intake::closed_source_history;
use super::payload::unrelated_attachment_payload;
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::storage as sql_error;

pub(super) async fn exclude_conflicts(
    tx: &mut Transaction<'_, Postgres>,
    target: &AssetConflictTarget<'_>,
    ignored_delivery: Option<Uuid>,
) -> Result<(), FundingError> {
    // 仅原equipment_recovered门已闭合且原handoff有明确Sent成功时排除该旧出站。
    // handoff 须仍为确认前的 enqueued，或已由确认事务以精确收口事实（failed/
    // recovery_equipment_recovered 与当前 fence 上的恢复来源处置）终结；只排除这一条原投递，
    // prepared/sending/manual_reconcile、任意其他 failed 及未知结果不在此例外内，也不改写旧义务。
    // 可信 Sent 证明与确认收回链、内部退款校验同一谓词，旧格式引用恢复证明同样放行。
    let recovered_handoff: Option<uuid::Uuid> =
        if target.kind == auction_domain::SettlementObligationKind::BuyerRefund {
            sqlx::query_scalar(concat!(
                r#"SELECT d.id
                   FROM public.settlement_recoveries r
                   JOIN public.settlement_obligation_fulfillments h
                     ON h.obligation_id=r.equipment_obligation_id
                   JOIN public.deliveries d ON d.id=h.delivery_id
                   JOIN public.settlement_recovery_actions a ON a.recovery_id=r.id
                     AND a.kind='recover_equipment_take'
                   WHERE r.buyer_refund_obligation_id=$1
                     AND r.stage='refunding'
                     AND r.resolution='equipment_recovered'
                     AND r.custody_proof_digest=a.proof_digest
                     AND a.execution_state='satisfied'
                     AND a.proof_digest IS NOT NULL
                     AND (h.execution_state='enqueued' OR "#,
                crate::database::settlement::fulfillment::equipment_recovered_closure_sql!("r"),
                r#")
                     AND "#,
                crate::database::settlement::fulfillment::recovered_handoff_sent_proof_sql!("d")
            ))
            .bind(target.obligation_id.as_uuid())
            .fetch_optional(&mut **tx)
            .await
            .map_err(sql_error)?
        } else {
            ignored_delivery
        };
    let conflict: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
                      SELECT 1
                      FROM public.settlement_obligations o
                      JOIN public.equipment e ON e.id=o.equipment_id
                      LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id=o.id
                      WHERE e.external_equipment_id=$1
                        AND e.world_id=$2
                        AND o.id<>$3
                        AND (f.delivery_id IS NULL OR f.delivery_id IS DISTINCT FROM $5)
                        AND (
                            (o.lot_id<>$4 AND COALESCE(f.execution_state,'pending')<>'satisfied')
                            OR f.execution_state IN ('prepared','enqueued','manual_reconcile')
                            OR (
                                f.execution_state='pending'
                                AND (
                                    f.delivery_id IS NOT NULL
                                    OR f.prepare_command_id IS NOT NULL
                                    OR f.activate_command_id IS NOT NULL
                                )
                            )
                        )
                  )
                  OR EXISTS(
                      SELECT 1
                      FROM public.settlement_recoveries r
                      JOIN public.settlement_obligations o ON o.id=r.payment_obligation_id
                      JOIN public.equipment e ON e.id=o.equipment_id
                      WHERE e.external_equipment_id=$1
                        AND e.world_id=$2
                        AND (
                            r.blocking_code IS NOT NULL
                            OR r.manual_reconcile_delivery_id IS NOT NULL
                            OR (r.lot_id<>$4 AND r.stage<>'completed')
                            OR (r.lot_id=$4 AND r.stage IN ('reclaiming','relist_pending'))
                        )
                  )
                  OR EXISTS(
                      SELECT 1
                      FROM public.settlement_recovery_actions a
                      JOIN public.settlement_recoveries r ON r.id=a.recovery_id
                      JOIN public.settlement_obligations o ON o.id=r.payment_obligation_id
                      JOIN public.equipment e ON e.id=o.equipment_id
                      WHERE e.external_equipment_id=$1
                        AND e.world_id=$2
                        AND a.execution_state<>'satisfied'
                  )"#,
    )
    .bind(target.external_equipment_id)
    .bind(target.equipment_world.as_str())
    .bind(target.obligation_id.as_uuid())
    .bind(target.lot_id.as_uuid())
    .bind(recovered_handoff)
    .fetch_one(&mut **tx)
    .await
    .map_err(sql_error)?;
    if conflict {
        return Err(FundingError::Unproven);
    }
    // 普通通知与入站处置用attachment_ids，而非settlement的装备secret引用；只排除已验证schema
    // 且不涉及目标EID/world的普通载荷，来源关联仍由后续SQL独立核对。
    // 必须完整扫描历史候选，但正文与附件逐行释放；内存只保留闭集验证通过的 UUID。
    // 扫描来源须覆盖统一投递判定的全部来源（`MailDeliveryOrigin::ALL`），漏掉的来源永远判为冲突。
    let mut unrelated_attachments = Vec::new();
    let mut deliveries = sqlx::query(
        r#"SELECT d.id,
                  d.command_id,
                  d.origin_kind,
                  d.origin_id,
                  d.payload
           FROM public.deliveries d
           WHERE d.origin_kind IN (
               'mail_intake','settlement_recovery_action','settlement_return_action',
               'operations_alert','lot_watch','seller_summary','season_reminder'
           )"#,
    )
    .fetch(&mut **tx);
    while let Some(delivery) = deliveries.try_next().await.map_err(sql_error)? {
        let payload: serde_json::Value = delivery.try_get("payload").map_err(sql_error)?;
        let origin: &str = delivery.try_get("origin_kind").map_err(sql_error)?;
        let origin_id: &str = delivery.try_get("origin_id").map_err(sql_error)?;
        let command_id: Uuid = delivery.try_get("command_id").map_err(sql_error)?;
        if unrelated_attachment_payload(&payload, origin, origin_id, command_id, target) {
            unrelated_attachments.push(delivery.try_get::<Uuid, _>("id").map_err(sql_error)?);
        }
    }
    drop(deliveries);
    // 当前标的的最终入库证明只闭合精确绑定的 take 及更早已完成 return；
    // 未知、后续动作或其他标的历史仍进入原有冲突门，不能仅凭 succeeded 放行。
    unrelated_attachments.extend(closed_source_history(tx, target).await?);
    // JSON只用于定位出站资产，不是资金授权。schema损坏/未知世界无法排除关联，保守阻断。
    // 成功delivery必须有对应的已满足fulfillment或recovery action；孤儿成功也不是清白。
    // 竞得装备正文只接受精确旧三/八变量或显式 v2 九变量；
    // 缺字段、未知模式/字段和非字符串值仍视为冲突。
    // 正文展示变量不携带资产，按生成方出现规则先剔除再比较键集合：竞得装备 COD 与载体付款的
    // payment_deadline，载体付款、卖方货款与买方退款的 item_name。曾因不认它们，库里一有新形状
    // 投递，全部内部履约与装备损失都判冲突。生成方只给价格明细形状的 COD、已写装备名的载体写
    // 期限，旧三变量 COD 或缺装备名的载体带期限仍判冲突。
    // 只缓存固定 SQL 文本，冲突事实仍在当前事务重新读取。
    static OUTGOING_QUERY: LazyLock<String> = LazyLock::new(|| {
        r#"SELECT EXISTS(
               SELECT 1
               FROM public.deliveries d
               WHERE d.id IS DISTINCT FROM $3
                 AND (
                   d.payload->'schema_version' IS DISTINCT FROM '1'::jsonb
                   OR d.payload_world IS NULL
                   OR d.payload_world NOT IN ('main','isekai')
                   OR EXISTS(
                       SELECT 1
                       FROM public.settlement_obligations origin_obligation
                       JOIN public.equipment origin_equipment
                         ON origin_equipment.id=origin_obligation.equipment_id
                       WHERE d.origin_kind='settlement'
                         AND d.origin_id=origin_obligation.id::text
                         AND origin_equipment.external_equipment_id=$1
                         AND origin_equipment.world_id=$2
                   )
                   OR EXISTS(
                       SELECT 1
                       FROM public.settlement_recovery_actions origin_action
                       JOIN public.settlement_recoveries origin_recovery
                         ON origin_recovery.id=origin_action.recovery_id
                       JOIN public.settlement_obligations origin_payment
                         ON origin_payment.id=origin_recovery.payment_obligation_id
                       JOIN public.equipment origin_equipment
                         ON origin_equipment.id=origin_payment.equipment_id
                       WHERE d.origin_id=origin_action.id::text
                         AND origin_equipment.external_equipment_id=$1
                         AND origin_equipment.world_id=$2
                   )
                   OR (
                     NOT (d.id=ANY($4))
                     AND (
                       d.origin_kind IS DISTINCT FROM 'settlement'
                       OR NOT COALESCE(
                           d.origin_id ~
                               '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                           AND d.origin_id<>'00000000-0000-0000-0000-000000000000',
                           false
                       )
                       OR d.payload IS DISTINCT FROM jsonb_build_object(
                           'schema_version',1,
                           'operation','send',
                           'world',d.payload->'world',
                           'template',d.payload->'template',
                           'variables',d.payload->'variables',
                           'credits',d.payload->'credits',
                           'cod',d.payload->'cod',
                           'equipment_secret_refs',d.payload->'equipment_secret_refs'
                       )
                       OR NOT COALESCE(
                           d.payload_template IN (
                               'settlement_winner_equipment_cod',
                               'settlement_payment_carrier',
                               'settlement_equipment_handoff',
                               'settlement_seller_payout',
                               'settlement_buyer_refund'
                           ),
                           false
                       )
                       OR jsonb_typeof(d.payload->'variables') IS DISTINCT FROM 'object'
                       OR (d.payload->'variables') - CASE d.payload_template
                           WHEN 'settlement_winner_equipment_cod'
                               THEN ARRAY['payment_deadline']
                           WHEN 'settlement_payment_carrier'
                               THEN ARRAY['item_name','payment_deadline']
                           WHEN 'settlement_seller_payout' THEN ARRAY['item_name','dispatch']
                           WHEN 'settlement_buyer_refund' THEN ARRAY['item_name','dispatch']
                           WHEN 'settlement_equipment_handoff' THEN ARRAY['dispatch']
                           ELSE ARRAY[]::text[] END
                         IS DISTINCT FROM CASE d.payload_template
                           WHEN 'settlement_winner_equipment_cod' THEN CASE
                               WHEN d.payload->'variables' ? 'pricing_mode' THEN CASE
                                   WHEN d.payload->'variables'->>'pricing_mode'='hold_curve_v2'
                                   THEN jsonb_build_object(
                                       'amount',d.payload->'variables'->'amount',
                                       'item_name',d.payload->'variables'->'item_name',
                                       'reference',d.payload->'variables'->'reference',
                                       'hold_ppm',d.payload->'variables'->'hold_ppm',
                                       'adjustment_ppm',d.payload->'variables'->'adjustment_ppm',
                                       'winning_bid',d.payload->'variables'->'winning_bid',
                                       'adjusted_price',d.payload->'variables'->'adjusted_price',
                                       'pool_credit',d.payload->'variables'->'pool_credit',
                                       'pricing_mode','hold_curve_v2'
                                   )
                                   ELSE NULL END
                               WHEN d.payload->'variables' ? 'hold_ppm' THEN jsonb_build_object(
                                   'amount',d.payload->'variables'->'amount',
                                   'item_name',d.payload->'variables'->'item_name',
                                   'reference',d.payload->'variables'->'reference',
                                   'hold_ppm',d.payload->'variables'->'hold_ppm',
                                   'adjustment_ppm',d.payload->'variables'->'adjustment_ppm',
                                   'winning_bid',d.payload->'variables'->'winning_bid',
                                   'adjusted_price',d.payload->'variables'->'adjusted_price',
                                   'pool_credit',d.payload->'variables'->'pool_credit'
                               )
                               ELSE jsonb_build_object(
                                   'amount',d.payload->'variables'->'amount',
                                   'item_name',d.payload->'variables'->'item_name',
                                   'reference',d.payload->'variables'->'reference'
                               ) END
                           WHEN 'settlement_equipment_handoff' THEN jsonb_build_object(
                               'item_name',d.payload->'variables'->'item_name',
                               'reference',d.payload->'variables'->'reference'
                           )
                           ELSE jsonb_build_object(
                               'amount',d.payload->'variables'->'amount',
                               'reference',d.payload->'variables'->'reference'
                           ) END
                       OR (
                           d.payload->'variables' ? 'payment_deadline'
                           AND CASE d.payload_template
                               WHEN 'settlement_winner_equipment_cod'
                                   THEN NOT d.payload->'variables' ? 'hold_ppm'
                               WHEN 'settlement_payment_carrier'
                                   THEN NOT d.payload->'variables' ? 'item_name'
                               ELSE false END
                       )
                       OR d.payload->'variables'->>'reference' IS DISTINCT FROM d.origin_id
                       OR EXISTS(
                           SELECT 1
                           FROM jsonb_each(CASE
                               WHEN jsonb_typeof(d.payload->'variables')='object'
                               THEN d.payload->'variables' ELSE '{}'::jsonb END) variable
                           WHERE jsonb_typeof(variable.value) IS DISTINCT FROM 'string'
                       )
                       OR NOT COALESCE(
                           jsonb_typeof(d.payload->'credits')='number'
                           AND d.payload->>'credits' ~ '^[0-9]+$'
                           AND d.payload->'credits'<='9223372036854775807'::jsonb,
                           false
                       )
                       OR NOT COALESCE(
                           jsonb_typeof(d.payload->'cod')='number'
                           AND d.payload->>'cod' ~ '^[0-9]+$'
                           AND d.payload->'cod'<='9223372036854775807'::jsonb,
                           false
                       )
                       OR CASE WHEN jsonb_typeof(d.payload->'equipment_secret_refs')='array'
                           THEN jsonb_array_length(d.payload->'equipment_secret_refs') ELSE -1 END
                           <> CASE WHEN d.payload_template IN (
                               'settlement_winner_equipment_cod','settlement_equipment_handoff'
                           ) THEN 1 ELSE 0 END
                       OR EXISTS(
                           SELECT 1
                           FROM jsonb_array_elements(
                               CASE WHEN jsonb_typeof(d.payload->'equipment_secret_refs')='array'
                                   THEN d.payload->'equipment_secret_refs' ELSE '[]'::jsonb END
                           ) ref
                           WHERE ref IS DISTINCT FROM jsonb_build_object(
                               'kind','auction_equipment_key','id',ref->'id'
                           )
                              OR jsonb_typeof(ref->'id') IS DISTINCT FROM 'string'
                              OR NOT EXISTS(
                                  SELECT 1
                                  FROM public.equipment known
                                  WHERE known.id::text=ref->>'id'
                                    AND known.world_id=d.payload_world
                              )
                              OR EXISTS(
                                  SELECT 1
                                  FROM public.equipment target
                                  WHERE target.id::text=ref->>'id'
                                    AND target.external_equipment_id=$1
                                    AND target.world_id=$2
                              )
                       )
                       OR d.payload_operation IS DISTINCT FROM 'send'
                     )
                   )
                 )
                 AND NOT EXISTS(
                   SELECT 1
                   FROM public.settlement_obligation_fulfillments f
                   WHERE f.delivery_id=d.id
                     AND f.execution_state='satisfied'
                     AND f.proof_digest IS NOT NULL
                 )
                 AND NOT EXISTS(
                   SELECT 1
                   FROM public.settlement_recovery_actions a
                   WHERE a.delivery_id=d.id
                     AND a.execution_state='satisfied'
                     AND a.proof_digest IS NOT NULL
                 )
                 AND NOT EXISTS(
                   SELECT 1
                   FROM public.settlement_return_takes returned
                   WHERE returned.take_delivery_id=d.id
                      OR returned.original_delivery_id=d.id
                 )
                 AND NOT EXISTS(
                   SELECT 1
                   FROM public.settlement_legacy_self_reconciliations r
                   WHERE /* trusted_legacy_self */
                 )
           )"#
        .replace(
            "/* trusted_legacy_self */",
            crate::database::delivery::legacy_self_proof::TRUSTED_DELIVERY_PREDICATE,
        )
    });
    // 替换值仅来自已审查的编译期 SQL 常量，不插入业务文本。
    let outgoing: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(OUTGOING_QUERY.as_str()))
        .bind(target.external_equipment_id)
        .bind(target.equipment_world.as_str())
        .bind(recovered_handoff)
        .bind(&unrelated_attachments)
        .fetch_one(&mut **tx)
        .await
        .map_err(sql_error)?;
    if outgoing {
        return Err(FundingError::Unproven);
    }
    Ok(())
}
