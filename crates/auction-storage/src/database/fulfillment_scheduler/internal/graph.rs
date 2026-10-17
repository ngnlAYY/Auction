//! 资金动作前核对原payment→handoff→seller及互斥refund，禁止借lot选择另一组义务。

use super::sql_error;
use auction_application::{FundingError, SettlementFulfillmentClaim};
use auction_domain::SettlementObligationKind;
use sqlx::{Postgres, Transaction};

pub(super) async fn validate_graph(
    tx: &mut Transaction<'_, Postgres>,
    c: &SettlementFulfillmentClaim,
) -> Result<(), FundingError> {
    if c.kind == SettlementObligationKind::UnsoldReturn {
        return Ok(());
    }
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.settlement_recoveries r
         JOIN public.lot_settlements l ON l.lot_id=r.lot_id
         JOIN public.settlement_obligations p ON p.id=r.payment_obligation_id
         JOIN public.settlement_obligations s ON s.id=r.seller_payout_obligation_id
         JOIN public.settlement_obligations b ON b.id=r.buyer_refund_obligation_id
         LEFT JOIN public.settlement_obligations h ON h.id=r.equipment_obligation_id
         WHERE r.lot_id=$1 AND r.auction_id=$2 AND r.equipment_id=$3
           AND p.lot_id=r.lot_id AND s.lot_id=r.lot_id AND b.lot_id=r.lot_id
           AND p.auction_id=r.auction_id AND s.auction_id=r.auction_id AND b.auction_id=r.auction_id
           AND p.equipment_id=r.equipment_id AND s.equipment_id=r.equipment_id AND b.equipment_id=r.equipment_id
           AND p.amount=l.buyer_amount AND b.amount=l.buyer_amount AND s.amount=l.seller_amount
           AND p.recipient_user_id=r.winner_user_id AND b.recipient_user_id=r.winner_user_id
           AND s.obligation_kind='seller_payout' AND b.obligation_kind='buyer_refund'
           AND p.prerequisite_obligation_id IS NULL AND b.prerequisite_obligation_id=p.id
           AND (SELECT count(*) FROM public.settlement_obligations WHERE lot_id=r.lot_id
                AND obligation_kind IN ('winner_equipment_cod','winner_payment_carrier_cod'))=1
           AND (p.id=$4 OR s.id=$4 OR b.id=$4 OR h.id=$4)
           AND ((r.world_id='main' AND p.obligation_kind='winner_equipment_cod'
                 AND r.equipment_obligation_id IS NULL AND s.prerequisite_obligation_id=p.id)
             OR (r.world_id='isekai' AND p.obligation_kind='winner_payment_carrier_cod'
                 AND h.obligation_kind='winner_equipment_handoff' AND h.lot_id=r.lot_id
                 AND h.auction_id=r.auction_id AND h.equipment_id=r.equipment_id
                 AND h.recipient_user_id=r.winner_user_id AND h.prerequisite_obligation_id=p.id
                 AND s.prerequisite_obligation_id=h.id)))"
    ).bind(c.lot_id.as_uuid()).bind(c.auction_id.as_uuid()).bind(c.equipment_id.as_uuid())
        .bind(c.obligation_id.as_uuid()).fetch_one(&mut **tx).await.map_err(sql_error)?;
    if !valid {
        return Err(FundingError::Unproven);
    }
    if matches!(
        c.kind,
        SettlementObligationKind::WinnerEquipmentCod
            | SettlementObligationKind::WinnerPaymentCarrierCod
            | SettlementObligationKind::WinnerEquipmentHandoff
    ) {
        let successors: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM public.settlement_recoveries r
             JOIN public.settlement_obligations s ON s.id=r.seller_payout_obligation_id
             JOIN public.settlement_obligation_fulfillments sf ON sf.obligation_id=s.id
             JOIN public.settlement_obligations b ON b.id=r.buyer_refund_obligation_id
             JOIN public.settlement_obligation_fulfillments bf ON bf.obligation_id=b.id
             WHERE r.lot_id=$1 AND r.resolution IS NULL AND r.blocking_code IS NULL
               AND s.business_state='blocked' AND sf.execution_state='pending'
               AND sf.delivery_id IS NULL AND sf.prepare_command_id IS NULL AND sf.activate_command_id IS NULL
               AND b.business_state='contingent' AND bf.execution_state='pending'
               AND bf.delivery_id IS NULL AND bf.prepare_command_id IS NULL AND bf.activate_command_id IS NULL)"
        ).bind(c.lot_id.as_uuid()).fetch_one(&mut **tx).await.map_err(sql_error)?;
        if !successors {
            return Err(FundingError::Conflict);
        }
    }
    Ok(())
}
