//! Serializable 装备损失事务；原始事实、人工责任、资金与正常卖家队列同时提交。

use crate::database::equipment_loss::{Error, snapshot};
use crate::database::{TrackedTransaction as Transaction, settlement_funding as funding};
use auction_application::settlement::{EquipmentLossCommand, EquipmentLossResult, FundingMutation};
use serde_json::json;
use sqlx::{Postgres, Row};

pub(super) async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    command: &EquipmentLossCommand,
) -> Result<Option<EquipmentLossResult>, Error> {
    let row = sqlx::query(
        "SELECT request_digest,result FROM public.settlement_equipment_losses WHERE command_id=$1",
    )
    .bind(command.command_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<Vec<u8>, _>("request_digest")?
        != command.request_digest().map_err(Error::Application)?
    {
        return Err(Error::Rejected);
    }
    let mut result: EquipmentLossResult = serde_json::from_value(row.try_get("result")?)?;
    result.replayed = true;
    Ok(Some(result))
}

pub(super) async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    command: &EquipmentLossCommand,
) -> Result<EquipmentLossResult, Error> {
    // 固定全局锁前缀；人工责任不需要伪造凭据绑定，但不能反向取得 generation/world/account。
    sqlx::query("SELECT generation FROM public.mail_runtime_generation WHERE scope='mail_cookie' FOR UPDATE")
        .execute(&mut **tx).await?;
    funding::lock_funding_worlds(tx).await?;
    let account =
        funding::lock_funding_account(tx, &command.funding_uid).await?.ok_or(Error::Rejected)?;
    if !account.ready() || !account.records_only() {
        return Err(Error::Rejected);
    }
    for query in [
        "SELECT id FROM public.auctions ORDER BY id FOR UPDATE",
        "SELECT id FROM public.settlement_recoveries ORDER BY id FOR UPDATE",
        "SELECT id FROM public.lots ORDER BY id FOR UPDATE",
        "SELECT id FROM public.equipment ORDER BY id FOR UPDATE",
        "SELECT id FROM public.users ORDER BY id FOR UPDATE",
        "SELECT id FROM public.settlement_obligations ORDER BY id FOR UPDATE",
        "SELECT id FROM public.settlement_recovery_actions ORDER BY id FOR UPDATE",
        "SELECT id FROM public.deliveries ORDER BY id FOR UPDATE",
        "SELECT obligation_id FROM public.settlement_obligation_fulfillments ORDER BY obligation_id FOR UPDATE",
    ] {
        sqlx::query(query).execute(&mut **tx).await?;
    }
    let snapshot = snapshot::load(tx, command.lot_id).await?;
    let p = &snapshot.preview;
    if !p.eligible
        || p.funding_uid.as_deref() != Some(command.funding_uid.as_str())
        || p.expected_recovery_version != command.expected_recovery_version
        || p.expected_payment_version != command.expected_payment_version
        || p.expected_delivery_fencing_version != command.expected_delivery_fencing_version
        || p.seller_amount != command.expected_seller_amount
    {
        return Err(Error::Rejected);
    }
    let admission = snapshot.admission()?;
    let recovery_id = admission.recovery.id;
    let payment_id = admission.payment.id;
    let delivery_id = admission.delivery.id;
    let seller_id = admission.seller.id;
    let refund_id = admission.refund.id;
    let split = auction_domain::equipment_loss_funding(
        snapshot.split()?,
        account.records_only(),
        command.buyer_compensation_external,
    )
    .map_err(|_| Error::Rejected)?;
    let digest = command.request_digest().map_err(Error::Application)?;
    let reference = format!("operator_equipment_loss_v1:{}", command.command_id);
    let result = EquipmentLossResult {
        command_id: command.command_id,
        lot_id: command.lot_id,
        payment_delivery_id: delivery_id,
        seller_obligation_id: seller_id,
        seller_amount: p.seller_amount,
        replayed: false,
    };
    // 事实表是唯一退休与命令重放来源；其快照不会通过预览或 CLI 泄露。
    sqlx::query("INSERT INTO public.settlement_equipment_losses(command_id,lot_id,recovery_id,
        payment_obligation_id,delivery_id,seller_obligation_id,refund_obligation_id,uid,actor,reason,
        buyer_compensation_external,expected_recovery_version,expected_payment_version,
        expected_delivery_fencing_version,buyer_amount,compensation_amount,seller_amount,tax_amount,
        request_digest,original_snapshot,result)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,true,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)")
        .bind(command.command_id).bind(command.lot_id).bind(recovery_id).bind(payment_id).bind(delivery_id)
        .bind(seller_id).bind(refund_id).bind(&command.funding_uid).bind(&command.actor).bind(&command.reason)
        .bind(i64::try_from(command.expected_recovery_version).map_err(|_|Error::Rejected)?)
        .bind(i64::try_from(command.expected_payment_version).map_err(|_|Error::Rejected)?)
        .bind(command.expected_delivery_fencing_version).bind(split.buyer.value()).bind(split.compensation.value())
        .bind(split.seller.value()).bind(split.tax.value()).bind(digest.as_slice()).bind(&snapshot.data)
        .bind(serde_json::to_value(&result)?).execute(&mut **tx).await?;
    let changed=sqlx::query("UPDATE public.deliveries SET status='failed',failure_code='equipment_destroyed',
        fencing_version=fencing_version+1,completed_at=clock_timestamp(),updated_at=clock_timestamp(),
        result_metadata=result_metadata || jsonb_build_object('equipment_loss_command_id',$3::text,'retired',true)
        WHERE id=$1 AND fencing_version=$2 AND status='manual_reconcile' AND lease_token IS NULL")
        .bind(delivery_id).bind(command.expected_delivery_fencing_version).bind(command.command_id)
        .execute(&mut **tx).await?;
    require_one(changed.rows_affected())?;
    let changed=sqlx::query("UPDATE public.settlement_obligation_fulfillments SET execution_state='satisfied',
        state_version=state_version+1,proof_kind='operator_equipment_loss_v1',proof_reference=$3,
        proof_digest=$4,proof_observed_at=clock_timestamp(),last_error_code=NULL,next_attempt_at=NULL,
        completed_at=clock_timestamp(),updated_at=clock_timestamp()
        WHERE obligation_id=$1 AND state_version=$2 AND execution_state='manual_reconcile' AND claim_token IS NULL")
        .bind(payment_id).bind(i64::try_from(command.expected_payment_version).map_err(|_|Error::Rejected)?)
        .bind(&reference).bind(digest.as_slice()).execute(&mut **tx).await?;
    require_one(changed.rows_affected())?;
    // 外部补偿是操作者承担的责任，不能把 refund 标记为已发送或已付款。
    let changed=sqlx::query("UPDATE public.settlement_obligation_fulfillments SET execution_state='failed',
        state_version=state_version+1,last_error_code='operator_external_buyer_compensation',
        proof_kind='operator_equipment_loss_v1',proof_reference=$2,proof_digest=$3,
        proof_observed_at=clock_timestamp(),next_attempt_at=NULL,completed_at=clock_timestamp(),updated_at=clock_timestamp()
        WHERE obligation_id=$1 AND execution_state='pending' AND state_version=1 AND delivery_id IS NULL AND claim_token IS NULL")
        .bind(refund_id).bind(&reference).bind(digest.as_slice()).execute(&mut **tx).await?;
    require_one(changed.rows_affected())?;
    funding::fund_equipment_loss(
        tx,
        &account,
        &FundingMutation { command_id: command.command_id, request_digest: digest },
        command.lot_id,
    )
    .await?;
    let changed=sqlx::query("UPDATE public.settlement_obligations SET business_state='ready',state_version=state_version+1,
        updated_at=clock_timestamp() WHERE id=$1 AND business_state='blocked' AND amount=$2")
        .bind(seller_id).bind(split.seller.value()).execute(&mut **tx).await?;
    require_one(changed.rows_affected())?;
    let changed=sqlx::query("UPDATE public.settlement_recoveries SET stage='completed',resolution=NULL,
        state_version=state_version+1,next_attempt_at=NULL,blocking_code=NULL,manual_reconcile_delivery_id=NULL,
        completed_at=clock_timestamp(),updated_at=clock_timestamp()
        WHERE id=$1 AND state_version=$2 AND stage='awaiting_payment' AND resolution IS NULL")
        .bind(recovery_id).bind(i64::try_from(command.expected_recovery_version).map_err(|_|Error::Rejected)?)
        .execute(&mut **tx).await?;
    require_one(changed.rows_affected())?;
    sqlx::query("INSERT INTO public.audit_records(id,occurred_at,actor_type,actor_id,action,entity_type,
        entity_id,result,source,metadata) VALUES($1,clock_timestamp(),'admin',$2,'auction.settlement.equipment_loss',
        'lot',$3,'success',$5,$4)")
        .bind(command.command_id).bind(&command.actor).bind(command.lot_id.to_string())
        .bind(auction_application::core::cli_audit_metadata(command.operator_identity.as_ref(), json!({"command":command,"payment_obligation_id":payment_id,"delivery_id":delivery_id,
            "seller_obligation_id":seller_id,"refund_obligation_id":refund_id,"recovery_id":recovery_id,
            "buyer_amount":split.buyer.value(),"compensation_amount":split.compensation.value(),
            "seller_amount":split.seller.value(),"tax_amount":split.tax.value(),
            "proof_kind":"operator_equipment_loss_v1","external_compensation_completed":false,
            "cod_collected":false,"seller_payment":"normal_queue","executor":"auction-cli"})))
        .bind(if command.operator_identity.is_some() { "cli" } else { "admin" })
        .execute(&mut **tx).await?;
    // 同一事务追加统一处置事实：投递被永久退役，读侧据此停止把它当作待发送失败。
    crate::database::delivery_disposition::append(
        tx,
        crate::database::delivery_disposition::DeliveryDispositionFact {
            delivery_id,
            fencing_version: command.expected_delivery_fencing_version + 1,
            kind: auction_domain::DeliveryDispositionKind::EquipmentLoss,
            source: auction_domain::DeliveryDispositionSource {
                equipment_loss_id: Some(command.command_id),
                ..Default::default()
            },
            actor_source: if command.operator_identity.is_some() {
                auction_domain::DeliveryDispositionActorSource::Cli
            } else {
                auction_domain::DeliveryDispositionActorSource::Admin
            },
            actor_id: &command.actor,
            reason: Some(&command.reason),
            reason_code: Some("equipment_destroyed"),
            detail: Some(&json!({
                "lot_id": command.lot_id,
                "recovery_id": recovery_id,
                "refund_obligation_id": refund_id,
                "buyer_compensation_external": true,
            })),
            command_id: None,
            request_digest: None,
        },
    )
    .await?;
    Ok(result)
}

fn require_one(changed: u64) -> Result<(), Error> {
    if changed == 1 { Ok(()) } else { Err(Error::Rejected) }
}
