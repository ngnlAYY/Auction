//! 装备损失处置的只读证明；只追加事实必须与当前退休投递、人工证明及资金来源同时匹配。

use super::*;
use std::sync::LazyLock;

/// `deliveries` 整行快照比较：先把每个 `timestamptz` 列归一化为 UTC 瞬间再转 JSON，
/// 再剔除本证明单独断言的变动列。`to_jsonb(行)` 会把时间戳渲染成会话 `TimeZone`
/// 的文本，直接整行比较会随会话时区翻转结论。快照缺失归一化时间键时由 `?&` 拒绝。
/// 迁移 `000094` 新增的 `payload_world`/`payload_operation`/`payload_template` 是
/// `payload` 的派生生成列，两侧一起剔除，使升级前写入的快照继续成立。
const DELIVERY_SNAPSHOT_EQUALITY: &str = r#"
           AND (x.original_snapshot->'delivery') ?& ARRAY[
               'lease_expires_at','claimed_at','sending_at','completed_at','activated_at','created_at']
           AND ((to_jsonb(d) || jsonb_build_object(
                   'lease_expires_at',coalesce(to_jsonb(d.lease_expires_at AT TIME ZONE 'UTC'),'null'::jsonb),
                   'claimed_at',coalesce(to_jsonb(d.claimed_at AT TIME ZONE 'UTC'),'null'::jsonb),
                   'sending_at',coalesce(to_jsonb(d.sending_at AT TIME ZONE 'UTC'),'null'::jsonb),
                   'completed_at',coalesce(to_jsonb(d.completed_at AT TIME ZONE 'UTC'),'null'::jsonb),
                   'activated_at',coalesce(to_jsonb(d.activated_at AT TIME ZONE 'UTC'),'null'::jsonb),
                   'created_at',coalesce(to_jsonb(d.created_at AT TIME ZONE 'UTC'),'null'::jsonb)))
               -'status'-'failure_code'-'fencing_version'-'completed_at'-'updated_at'-'result_metadata'-'payload_world'-'payload_operation'-'payload_template')=
               ((x.original_snapshot->'delivery' || jsonb_build_object(
                   'lease_expires_at',coalesce(to_jsonb((x.original_snapshot#>>'{delivery,lease_expires_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
                   'claimed_at',coalesce(to_jsonb((x.original_snapshot#>>'{delivery,claimed_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
                   'sending_at',coalesce(to_jsonb((x.original_snapshot#>>'{delivery,sending_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
                   'completed_at',coalesce(to_jsonb((x.original_snapshot#>>'{delivery,completed_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
                   'activated_at',coalesce(to_jsonb((x.original_snapshot#>>'{delivery,activated_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
                   'created_at',coalesce(to_jsonb((x.original_snapshot#>>'{delivery,created_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb)))
               -'status'-'failure_code'-'fencing_version'-'completed_at'-'updated_at'-'result_metadata'-'payload_world'-'payload_operation'-'payload_template')
"#;

pub(crate) async fn trusted_losses(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
) -> Result<Vec<(Uuid, Uuid)>, FundingError> {
    // 只缓存固定 SQL 文本，不缓存任何 UID 的证明结果；拼接值仅为已审查的编译期常量。
    static QUERY: LazyLock<String> = LazyLock::new(|| {
        "SELECT x.lot_id,x.delivery_id FROM public.settlement_equipment_losses x
         JOIN public.deliveries d ON d.id=x.delivery_id
         JOIN public.settlement_obligation_fulfillments pf ON pf.obligation_id=x.payment_obligation_id
         JOIN public.settlement_obligation_fulfillments bf ON bf.obligation_id=x.refund_obligation_id
         JOIN public.settlement_recoveries r ON r.id=x.recovery_id
         JOIN public.settlement_funding_lots fl ON fl.lot_id=x.lot_id
         WHERE x.uid=$1 AND fl.uid=x.uid AND fl.payment_obligation_id=x.payment_obligation_id
           AND fl.source_kind='operator_equipment_loss' AND fl.disposition IN ('seller','retained')
           AND d.status='failed' AND d.failure_code='equipment_destroyed'
           AND d.fencing_version=x.expected_delivery_fencing_version+1 AND d.external_reference IS NULL
           AND d.result_metadata=(x.original_snapshot#>'{delivery,result_metadata}')
             ||jsonb_build_object('equipment_loss_command_id',x.command_id::text,'retired',true)
           /* delivery_row_equality */
           AND pf.delivery_id=x.delivery_id AND pf.execution_state='satisfied'
           AND pf.proof_kind='operator_equipment_loss_v1' AND pf.proof_digest=x.request_digest
           AND pf.proof_reference='operator_equipment_loss_v1:'||x.command_id::text
           AND bf.delivery_id IS NULL AND bf.execution_state='failed'
           AND bf.last_error_code='operator_external_buyer_compensation'
           AND bf.proof_kind=pf.proof_kind AND bf.proof_reference=pf.proof_reference AND bf.proof_digest=x.request_digest
           AND r.stage='completed' AND r.resolution IS NULL AND r.relisted_lot_id IS NULL
         ORDER BY x.lot_id"
            .replace("/* delivery_row_equality */", DELIVERY_SNAPSHOT_EQUALITY)
    });
    sqlx::query_as(sqlx::AssertSqlSafe(QUERY.as_str()))
        .bind(uid)
        .fetch_all(&mut **tx)
        .await
        .map_err(funding::storage)
}

/// 集成测试入口：只返回静态 SQL 文本，别名固定为 `d`、`x`，不暴露任何证明结果或凭据。
#[cfg(feature = "test-db")]
pub fn delivery_snapshot_equality_sql() -> &'static str {
    DELIVERY_SNAPSHOT_EQUALITY
}
