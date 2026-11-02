//! 装备损失处置的只读证明；只追加事实必须与当前退休投递、人工证明及资金来源同时匹配。

use super::*;

pub(crate) async fn trusted_losses(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
) -> Result<Vec<(Uuid, Uuid)>, FundingError> {
    sqlx::query_as(
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
           AND (to_jsonb(d)-'status'-'failure_code'-'fencing_version'-'completed_at'-'updated_at'-'result_metadata')
             =((x.original_snapshot->'delivery')-'status'-'failure_code'-'fencing_version'-'completed_at'-'updated_at'-'result_metadata')
           AND pf.delivery_id=x.delivery_id AND pf.execution_state='satisfied'
           AND pf.proof_kind='operator_equipment_loss_v1' AND pf.proof_digest=x.request_digest
           AND pf.proof_reference='operator_equipment_loss_v1:'||x.command_id::text
           AND bf.delivery_id IS NULL AND bf.execution_state='failed'
           AND bf.last_error_code='operator_external_buyer_compensation'
           AND bf.proof_kind=pf.proof_kind AND bf.proof_reference=pf.proof_reference AND bf.proof_digest=x.request_digest
           AND r.stage='completed' AND r.resolution IS NULL AND r.relisted_lot_id IS NULL
         ORDER BY x.lot_id",
    ).bind(uid).fetch_all(&mut **tx).await.map_err(funding::storage)
}
