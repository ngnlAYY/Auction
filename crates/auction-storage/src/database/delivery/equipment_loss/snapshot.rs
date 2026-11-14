//! 损失处置快照；资格判定由领域 `EquipmentLossAdmission` 完成，原始快照仅供事实表留档。

use crate::database::TrackedTransaction as Transaction;
use crate::database::delivery::equipment_loss::Error;
use auction_application::settlement::{EquipmentLossPreview, FundingError};
use auction_domain::{EquipmentLossAdmission, FundingSplit};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

pub(super) struct Snapshot {
    /// 原始事实：随命令永久保存为 `original_snapshot`，不通过预览或 CLI 泄露。
    pub data: Value,
    /// 缺少完整 COD 结算图时为 `None`；此时预览已含 `missing_settled_equipment_cod_graph`。
    pub admission: Option<EquipmentLossAdmission>,
    pub preview: EquipmentLossPreview,
}

impl Snapshot {
    /// 冻结金额装配；提交事务仍以命令参数与账户模式做最终校验。
    pub fn split(&self) -> Result<FundingSplit, Error> {
        self.admission.as_ref().ok_or(Error::Rejected)?.funding_split().map_err(|_| Error::Rejected)
    }

    /// 仅在已确认 `eligible` 后取用；缺失快照属于内部矛盾。
    pub fn admission(&self) -> Result<&EquipmentLossAdmission, Error> {
        self.admission.as_ref().ok_or(Error::Rejected)
    }
}

pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    lot_id: Uuid,
) -> Result<Snapshot, Error> {
    let data: Option<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('auction',to_jsonb(a),'lot',to_jsonb(l),
          'equipment',to_jsonb(e),'settlement',to_jsonb(ls),'recovery',to_jsonb(r),
          'payment',to_jsonb(p),'payment_fulfillment',to_jsonb(pf),
          'seller',to_jsonb(s),'seller_fulfillment',to_jsonb(sf),
          'refund',to_jsonb(b),'refund_fulfillment',to_jsonb(bf),'delivery',to_jsonb(d),
          'funding_accounts',(SELECT COALESCE(jsonb_agg(jsonb_build_object('uid',uid,
             'status',status,'accounting_mode',accounting_mode)),'[]'::jsonb)
             FROM public.settlement_funding_accounts),
          'conflict',(
             EXISTS(SELECT 1 FROM public.settlement_funding_lots WHERE lot_id=l.id)
             OR EXISTS(SELECT 1 FROM public.settlement_funding_allocations WHERE lot_id=l.id)
             OR EXISTS(SELECT 1 FROM public.settlement_equipment_losses WHERE lot_id=l.id OR delivery_id=d.id)
             OR EXISTS(SELECT 1 FROM public.settlement_recovery_actions WHERE recovery_id=r.id)
             OR EXISTS(SELECT 1 FROM public.compensation_pool_reversals WHERE lot_id=l.id)
             OR EXISTS(SELECT 1 FROM public.delivery_sent_bindings WHERE delivery_id=d.id)
             OR EXISTS(SELECT 1 FROM public.settlement_funding_receipts WHERE payment_obligation_id=p.id OR delivery_id=d.id)
             OR EXISTS(SELECT 1 FROM public.settlement_funding_sources WHERE source_key=p.id)
             OR EXISTS(SELECT 1 FROM public.settlement_internal_commands WHERE obligation_id IN (p.id,s.id,b.id))
             OR EXISTS(SELECT 1 FROM public.settlement_legacy_self_reconciliations WHERE obligation_id IN (p.id,s.id,b.id))
             OR EXISTS(SELECT 1 FROM public.settlement_legacy_bot_seller_claims WHERE lot_id=l.id)
             OR EXISTS(SELECT 1 FROM public.deliveries WHERE id<>d.id AND origin_kind='settlement' AND origin_id IN (p.id::text,s.id::text,b.id::text))
             OR (SELECT count(*) FROM public.settlement_obligations WHERE lot_id=l.id)<>3))
         FROM public.lots l JOIN public.auctions a ON a.id=l.auction_id
         JOIN public.equipment e ON e.id=l.equipment_id
         JOIN public.lot_settlements ls ON ls.lot_id=l.id
         JOIN public.auction_settlements ast ON ast.auction_id=a.id
         JOIN public.settlement_recoveries r ON r.lot_id=l.id AND r.auction_id=a.id
         JOIN public.settlement_obligations p ON p.id=r.payment_obligation_id
         JOIN public.settlement_obligation_fulfillments pf ON pf.obligation_id=p.id
         JOIN public.settlement_obligations s ON s.id=r.seller_payout_obligation_id
         JOIN public.settlement_obligation_fulfillments sf ON sf.obligation_id=s.id
         JOIN public.settlement_obligations b ON b.id=r.buyer_refund_obligation_id
         JOIN public.settlement_obligation_fulfillments bf ON bf.obligation_id=b.id
         JOIN public.deliveries d ON d.id=pf.delivery_id
         WHERE l.id=$1",
    )
    .bind(lot_id)
    .fetch_optional(&mut **tx)
    .await?;
    let mut preview = EquipmentLossPreview {
        eligible: false,
        blockers: Vec::new(),
        lot_id,
        funding_uid: None,
        expected_recovery_version: 0,
        expected_payment_version: 0,
        expected_delivery_fencing_version: 0,
        seller_amount: 0,
        buyer_amount: 0,
    };
    let Some(data) = data else {
        preview.blockers.push("missing_settled_equipment_cod_graph".into());
        return Ok(Snapshot { data: Value::Null, admission: None, preview });
    };
    let admission = EquipmentLossAdmission::deserialize(&data)?;
    preview.expected_recovery_version =
        u64::try_from(admission.recovery.state_version).unwrap_or(0);
    preview.expected_payment_version =
        u64::try_from(admission.payment_fulfillment.state_version).unwrap_or(0);
    preview.expected_delivery_fencing_version = admission.delivery.fencing_version;
    preview.seller_amount = admission.settlement.seller_amount;
    preview.buyer_amount = admission.settlement.buyer_amount;
    if admission.funding_accounts.len() == 1 {
        preview.funding_uid = admission.funding_accounts.first().map(|account| account.uid.clone());
    }
    preview
        .blockers
        .extend(admission.evaluate().into_iter().map(|blocker| blocker.as_str().to_owned()));
    let mut snapshot = Snapshot { data, admission: Some(admission), preview };
    if snapshot.preview.blockers.is_empty() {
        let admission = snapshot.admission()?;
        match crate::database::settlement::fulfillment::exclude_equipment_loss_conflicts(
            tx,
            lot_id,
            admission.payment.id,
            &admission.equipment.external_equipment_id,
            admission.delivery.id,
        )
        .await
        {
            Ok(()) => {}
            Err(FundingError::Unproven | FundingError::Conflict | FundingError::Invalid) => {
                snapshot.preview.blockers.push("conflicting_asset_operation".into());
            }
            Err(error) => return Err(Error::Funding(error)),
        }
    }
    snapshot.preview.eligible = snapshot.preview.blockers.is_empty();
    Ok(snapshot)
}
