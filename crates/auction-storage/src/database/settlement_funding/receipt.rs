//! 新COD来源的精确账号/邮件绑定；唯一协议观察来自A，金额来自原冻结业务。

use super::*;
use auction_application::{FundingCollectedReceipt, canonical_settlement_fulfillment_subject};
use auction_domain::{SettlementId, SettlementObligationKind, SettlementWorld};
use serde_json::json;

pub(super) async fn validate_receipt(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    receipt: &FundingCollectedReceipt<'_>,
    payment: Uuid,
    buyer: i64,
) -> Result<(), FundingError> {
    let evidence = receipt.evidence;
    let sent = evidence.sent_collection.as_ref().ok_or(FundingError::Unproven)?;
    let cash = evidence.main_cash.as_ref().ok_or(FundingError::Unproven)?;
    if !evidence.validate()
        || receipt.payment_obligation_id != payment
        || receipt.buyer_credits != buyer
        || sent.binding.authenticated_uid != account.uid()
        || sent.binding.world != SettlementWorld::Main
        || cash.binding.world != SettlementWorld::Main
        || cash.binding.authenticated_uid != account.uid()
        || !auction_application::valid_external_positive_id(&sent.external_mail_id)
    {
        return Err(FundingError::Unproven);
    }
    // generation/credential锁由调用者按统一前缀取得；此处只复核全快照实时时效，不倒锁。
    crate::validate_mail_evidence_time(tx, evidence).await.map_err(evidence_error)?;
    let row = sqlx::query(
        r#"SELECT o.obligation_kind,o.amount,o.carrier_credit,o.equipment_id,
        e.name AS equipment_name,d.payload,d.recipient,d.result_metadata
        FROM public.settlement_obligations o
        JOIN public.equipment e ON e.id=o.equipment_id
        JOIN public.settlement_obligation_fulfillments f ON f.obligation_id=o.id
        JOIN public.deliveries d ON d.id=f.delivery_id
        WHERE o.id=$1 AND d.id=$2 AND f.execution_state='satisfied'
          AND f.proof_kind='mooglemail_sent_collected' AND f.proof_reference=$3
          AND f.proof_digest IS NOT NULL AND d.status='succeeded'
          AND d.origin_kind='settlement' AND d.origin_id=o.id::text
          AND d.external_reference=('mooglemail:main:send:' || $3)
          AND d.command_id=f.prepare_command_id AND d.request_digest=f.prepare_request_digest
          AND d.activation_command_id=f.activate_command_id
          AND d.activation_request_digest=f.activate_request_digest
          AND d.sending_at > (SELECT max(created_at) FROM public.settlement_funding_sources
                             WHERE uid=$4 AND kind='reconciliation')"#,
    )
    .bind(payment)
    .bind(receipt.delivery_id)
    .bind(&sent.external_mail_id)
    .bind(account.uid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    let kind: String = row.try_get("obligation_kind").map_err(storage)?;
    let carrier: i64 = row.try_get("carrier_credit").map_err(storage)?;
    let amount: i64 = row.try_get("amount").map_err(storage)?;
    let (kind, template) = match kind.as_str() {
        "winner_equipment_cod" if carrier == 0 => {
            (SettlementObligationKind::WinnerEquipmentCod, "settlement_winner_equipment_cod")
        }
        "winner_payment_carrier_cod" if carrier == 1 => {
            (SettlementObligationKind::WinnerPaymentCarrierCod, "settlement_payment_carrier")
        }
        _ => return Err(FundingError::Unproven),
    };
    let cod = buyer.checked_add(carrier).ok_or(FundingError::Invalid)?;
    if amount != buyer
        || buyer <= 0
        || row.try_get::<String, _>("recipient").map_err(storage)? != sent.recipient
        || canonical_settlement_fulfillment_subject(kind, SettlementId::from_uuid(payment))
            .as_deref()
            != Some(sent.subject.as_str())
    {
        return Err(FundingError::Unproven);
    }
    let mut variables = json!({"reference":payment.to_string(),"amount":cod.to_string()});
    let equipment = if carrier == 0 {
        variables["item_name"] =
            row.try_get::<String, _>("equipment_name").map_err(storage)?.into();
        json!([{"kind":"auction_equipment_key","id":row.try_get::<Uuid,_>("equipment_id").map_err(storage)?.to_string()}])
    } else {
        json!([])
    };
    let expected = json!({"schema_version":1,"operation":"send","world":"main","template":template,
        "variables":variables,"credits":carrier,"cod":cod,"equipment_secret_refs":equipment});
    let payload: serde_json::Value = row.try_get("payload").map_err(storage)?;
    let metadata: serde_json::Value = row.try_get("result_metadata").map_err(storage)?;
    if payload != expected
        || metadata["operation"] != "send"
        || metadata["template"] != template
        || metadata["external_reference_proven"] != true
        || metadata["sent_verified"] != true
        || metadata["proof"] != "sent_exact_payload_v1"
        || metadata["settlement_obligation_id"] != payment.to_string()
    {
        return Err(FundingError::Unproven);
    }
    Ok(())
}

pub(super) async fn store_receipt(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    receipt: &FundingCollectedReceipt<'_>,
) -> Result<(), FundingError> {
    let sent = receipt.evidence.sent_collection.as_ref().ok_or(FundingError::Unproven)?;
    let cash = receipt.evidence.main_cash.as_ref().ok_or(FundingError::Unproven)?;
    let evidence = json!({"credential_id":sent.binding.credential_id,
        "credential_version":sent.binding.credential_version,"runtime_generation":sent.binding.runtime.generation,
        "uid":sent.binding.authenticated_uid,"player_name":sent.binding.player_name,
        "protocol":sent.binding.protocol_version,"buyer_credits":receipt.buyer_credits,
        "recipient":sent.recipient,"subject":sent.subject,"sent_at":sent.sent_at,
        "collected":sent.collected,"remaining_credits":sent.remaining_credits,"cod_pending":sent.cod_pending,
        "remaining_equipment_ids":sent.remaining_equipment_ids,
        "receipt_observed_at":sent.observed_at,"receipt_digest":sent.digest,
        "binding_digest":sent.binding.digest,"cash_credits":cash.credits,
        "cash_observed_at":cash.observed_at,"cash_digest":cash.digest,"snapshot_digest":receipt.evidence.digest});
    sqlx::query("INSERT INTO public.settlement_funding_receipts(source_id,uid,payment_obligation_id,delivery_id,external_mail_id,evidence) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(source).bind(&sent.binding.authenticated_uid).bind(receipt.payment_obligation_id)
        .bind(receipt.delivery_id).bind(&sent.external_mail_id).bind(evidence)
        .execute(&mut **tx).await.map_err(storage)?;
    Ok(())
}
