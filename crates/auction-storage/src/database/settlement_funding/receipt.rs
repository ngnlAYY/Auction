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
    validate_receipt_boundary(tx, account, receipt, payment, buyer, false).await
}

/// 开户前已收款只能重认既有隔离责任；复用原付款精确协议，不创建第二个收款来源。
pub(super) async fn validate_legacy_receipt(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    receipt: &FundingCollectedReceipt<'_>,
    payment: Uuid,
    buyer: i64,
) -> Result<(), FundingError> {
    validate_receipt_boundary(tx, account, receipt, payment, buyer, true).await
}

async fn validate_receipt_boundary(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    receipt: &FundingCollectedReceipt<'_>,
    payment: Uuid,
    buyer: i64,
    legacy: bool,
) -> Result<(), FundingError> {
    let evidence = receipt.evidence;
    let sent = evidence.sent_collection.as_ref().ok_or(FundingError::Unproven)?;
    let cash = evidence.main_cash.as_ref();
    if !evidence.validate()
        || receipt.payment_obligation_id != payment
        || receipt.buyer_credits != buyer
        || sent.binding.authenticated_uid != account.uid()
        || sent.binding.world != SettlementWorld::Main
        || if account.records_only() {
            cash.is_some()
        } else {
            cash.is_none_or(|cash| {
                cash.binding.world != SettlementWorld::Main
                    || cash.binding.authenticated_uid != account.uid()
            })
        }
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
          AND (($5 AND d.sending_at <= (SELECT min(created_at) FROM public.settlement_funding_sources
                             WHERE uid=$4 AND kind=$6))
            OR (NOT $5 AND d.sending_at > (SELECT max(created_at) FROM public.settlement_funding_sources
                             WHERE uid=$4 AND kind=$6)))"#,
    )
    .bind(payment)
    .bind(receipt.delivery_id)
    .bind(&sent.external_mail_id)
    .bind(account.uid())
    .bind(legacy)
    .bind(if account.records_only() {"record_opening"} else {"reconciliation"})
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
    let exact = metadata["proof"] == "sent_exact_payload_v1"
        && metadata["sent_verified"] == true
        && metadata["settlement_obligation_id"] == payment.to_string();
    // 开户前的既有责任允许使用原维护恢复事实；新收款仍只接受发送时的 exact 证明。
    // 此分支不升级旧 proof，也不代替本次同账号、精确 MID 的已收款联合证据。
    let recovered = legacy
        && legacy_recovered_proof(&metadata, &sent.external_mail_id, &sent.recipient, cod, carrier);
    if payload != expected
        || metadata["operation"] != "send"
        || metadata["template"] != template
        || metadata["external_reference_proven"] != true
        || !(exact || recovered)
    {
        return Err(FundingError::Unproven);
    }
    Ok(())
}

/// 原维护恢复的闭集事实：恢复时可以尚未付款，本次收款仍由 opaque Sent 证明。
fn legacy_recovered_proof(
    metadata: &serde_json::Value,
    mid: &str,
    recipient: &str,
    cod: i64,
    carrier: i64,
) -> bool {
    let Some(collected) = metadata["collected"].as_bool() else {
        return false;
    };
    let (pending, credits, equipment_count) =
        if collected { (0, 0, 0) } else { (cod, carrier, i64::from(carrier == 0)) };
    let sweep = &metadata["mailbox_sweep"];
    let observed = |value: &serde_json::Value| {
        value.as_str().is_some_and(|value| chrono::DateTime::parse_from_rfc3339(value).is_ok())
    };
    metadata["proof"] == "sent_recovered_reference_v1"
        && metadata["sent_recovered"] == true
        && metadata["recovered_mid"] == mid
        && metadata["recipient"] == recipient
        && metadata["cod_pending"] == pending
        && observed(&metadata["observed_at"])
        && sweep["mid"] == mid
        && sweep["collected"] == collected
        && sweep["cod_pending"] == pending
        && sweep["remaining_credits"] == credits
        && sweep["remaining_equipment_count"] == equipment_count
        && observed(&sweep["observed_at"])
}

pub(super) async fn store_receipt(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    receipt: &FundingCollectedReceipt<'_>,
) -> Result<(), FundingError> {
    let sent = receipt.evidence.sent_collection.as_ref().ok_or(FundingError::Unproven)?;
    let cash = receipt.evidence.main_cash.as_ref();
    let evidence = json!({"credential_id":sent.binding.credential_id,
        "credential_version":sent.binding.credential_version,"runtime_generation":sent.binding.runtime.generation,
        "uid":sent.binding.authenticated_uid,"player_name":sent.binding.player_name,
        "protocol":sent.binding.protocol_version,"buyer_credits":receipt.buyer_credits,
        "recipient":sent.recipient,"subject":sent.subject,"sent_at":sent.sent_at,
        "collected":sent.collected,"remaining_credits":sent.remaining_credits,"cod_pending":sent.cod_pending,
        "remaining_equipment_ids":sent.remaining_equipment_ids,
        "receipt_observed_at":sent.observed_at,"receipt_digest":sent.digest,
        "binding_digest":sent.binding.digest,"cash_credits":cash.map(|cash|cash.credits),
        "cash_observed_at":cash.map(|cash|cash.observed_at),"cash_digest":cash.map(|cash|cash.digest),"snapshot_digest":receipt.evidence.digest});
    sqlx::query("INSERT INTO public.settlement_funding_receipts(source_id,uid,payment_obligation_id,delivery_id,external_mail_id,evidence) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(source).bind(&sent.binding.authenticated_uid).bind(receipt.payment_obligation_id)
        .bind(receipt.delivery_id).bind(&sent.external_mail_id).bind(evidence)
        .execute(&mut **tx).await.map_err(storage)?;
    Ok(())
}
