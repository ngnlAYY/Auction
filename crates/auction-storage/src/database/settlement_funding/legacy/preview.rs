//! 单目标的安全冻结快照与精确 Sent 目标；不读取凭据、正文或装备 key。

use auction_application::{
    FundingError, LegacyBotSellerPreview, canonical_settlement_fulfillment_subject,
};
use auction_domain::{FundingSplit, SettlementAmount, SettlementId, SettlementObligationKind};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::super::{invalid, storage};

pub(super) struct Target {
    pub preview: LegacyBotSellerPreview,
    pub facts: Value,
    pub seller_name: String,
}

pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    obligation: Uuid,
    uid: &str,
) -> Result<Target, FundingError> {
    let facts: Value = sqlx::query_scalar(
        "SELECT to_jsonb(x) FROM (
         SELECT s.id AS obligation_id,s.auction_id,s.lot_id,s.equipment_id,s.amount,
           s.recipient_game_identity AS seller_name,s.recipient_user_id,
           s.business_state AS seller_business,s.prerequisite_obligation_id AS seller_prerequisite,
           sf.execution_state AS seller_state,sf.state_version,sf.delivery_id AS seller_delivery,
           sf.prepare_command_id AS seller_prepare,sf.activate_command_id AS seller_activate,
           sf.proof_kind AS seller_proof,
           (sf.claim_token IS NOT NULL AND sf.claim_expires_at>clock_timestamp()) AS seller_claim_active,
           r.id AS recovery_id,r.world_id,r.resolution,r.blocking_code,
           l.buyer_amount,l.compensation_amount,l.seller_amount,l.tax_amount,
           p.id AS payment_obligation_id,p.obligation_kind AS payment_kind,p.amount AS payment_amount,
           pf.execution_state AS payment_state,pf.proof_kind AS payment_proof,
           pf.proof_reference AS payment_reference,pf.proof_digest AS payment_digest,
           pf.delivery_id AS payment_delivery_id,d.status AS payment_delivery_status,
           d.recipient AS payment_recipient,d.external_reference AS payment_external_reference,
           d.payload AS payment_payload,d.result_metadata AS payment_metadata,
           d.command_id,d.request_digest,d.activation_command_id,d.activation_request_digest,
           pf.prepare_command_id,pf.prepare_request_digest,pf.activate_command_id,pf.activate_request_digest,
           h.id AS handoff_id,hf.execution_state AS handoff_state,hf.proof_digest AS handoff_digest,
           b.id AS refund_id,b.business_state AS refund_business,bf.execution_state AS refund_state,
           bf.delivery_id AS refund_delivery,bf.proof_kind AS refund_proof,
           COALESCE(a.state_version,0) AS account_version,COALESCE(a.status,'unreconciled') AS account_status,
           COALESCE(a.accounting_mode,'strict_cash') AS accounting_mode,COALESCE(a.policy_version,0) AS policy_version,
           COALESCE(q.amount,0) AS quarantine_amount,q.state AS quarantine_state,
           EXISTS(SELECT 1 FROM public.settlement_funding_lots fl WHERE fl.lot_id=s.lot_id) AS already_funded,
           EXISTS(SELECT 1 FROM public.settlement_funding_receipts fr WHERE fr.payment_obligation_id=p.id
             OR fr.delivery_id=d.id OR (fr.uid=$2 AND fr.external_mail_id=pf.proof_reference)) AS already_receipted,
           EXISTS(SELECT 1 FROM public.settlement_legacy_bot_seller_claims c WHERE c.lot_id=s.lot_id
             OR c.payment_obligation_id=p.id OR c.payment_delivery_id=d.id
             OR (c.uid=$2 AND c.external_mail_id=pf.proof_reference)) AS already_claimed
         FROM public.settlement_obligations s
         JOIN public.settlement_obligation_fulfillments sf ON sf.obligation_id=s.id
         JOIN public.settlement_recoveries r ON r.seller_payout_obligation_id=s.id
           AND r.lot_id=s.lot_id AND r.auction_id=s.auction_id AND r.equipment_id=s.equipment_id
         JOIN public.lot_settlements l ON l.lot_id=s.lot_id
         JOIN public.settlement_obligations p ON p.id=r.payment_obligation_id
           AND p.lot_id=s.lot_id AND p.auction_id=s.auction_id AND p.equipment_id=s.equipment_id
         JOIN public.settlement_obligation_fulfillments pf ON pf.obligation_id=p.id
         JOIN public.deliveries d ON d.id=pf.delivery_id AND d.origin_kind='settlement' AND d.origin_id=p.id::text
         JOIN public.settlement_obligations b ON b.id=r.buyer_refund_obligation_id
           AND b.lot_id=s.lot_id AND b.equipment_id=s.equipment_id AND b.prerequisite_obligation_id=p.id
         JOIN public.settlement_obligation_fulfillments bf ON bf.obligation_id=b.id
         LEFT JOIN public.settlement_obligations h ON h.id=r.equipment_obligation_id
         LEFT JOIN public.settlement_obligation_fulfillments hf ON hf.obligation_id=h.id
         LEFT JOIN public.settlement_funding_accounts a ON a.uid=$2
         LEFT JOIN public.settlement_funding_allocations q ON q.uid=$2 AND q.purpose='quarantine' AND q.lot_id IS NULL
         WHERE s.id=$1 AND s.obligation_kind='seller_payout') x"
    ).bind(obligation).bind(uid).fetch_optional(&mut **tx).await.map_err(storage)?
        .ok_or(FundingError::Unproven)?;
    let seller_name = text(&facts, "seller_name")?;
    let payment = id(&facts, "payment_obligation_id")?;
    let mid = text(&facts, "payment_reference")?;
    let kind = match facts["payment_kind"].as_str() {
        Some("winner_equipment_cod") if facts["world_id"] == "main" => {
            SettlementObligationKind::WinnerEquipmentCod
        }
        Some("winner_payment_carrier_cod") if facts["world_id"] == "isekai" => {
            SettlementObligationKind::WinnerPaymentCarrierCod
        }
        _ => return Err(FundingError::Unproven),
    };
    let expected_prerequisite = if facts["world_id"] == "main" {
        payment
    } else {
        if facts["handoff_state"] != "satisfied" || facts["handoff_digest"].is_null() {
            return Err(FundingError::Unproven);
        }
        id(&facts, "handoff_id")?
    };
    if facts["seller_business"] != "ready"
        || facts["seller_state"] != "pending"
        || facts["seller_claim_active"] != false
        || !facts["recipient_user_id"].is_null()
        || !facts["seller_delivery"].is_null()
        || !facts["seller_prepare"].is_null()
        || !facts["seller_activate"].is_null()
        || !facts["seller_proof"].is_null()
        || id(&facts, "seller_prerequisite")? != expected_prerequisite
        || facts["payment_state"] != "satisfied"
        || facts["payment_proof"] != "mooglemail_sent_collected"
        || facts["payment_digest"].is_null()
        || facts["payment_delivery_status"] != "succeeded"
        || facts["refund_business"] != "contingent"
        || facts["refund_state"] != "pending"
        || !facts["refund_delivery"].is_null()
        || !facts["refund_proof"].is_null()
        || !facts["resolution"].is_null()
        || !facts["blocking_code"].is_null()
        || facts["already_funded"] != false
        || facts["already_receipted"] != false
        || facts["already_claimed"] != false
        || facts["command_id"] != facts["prepare_command_id"]
        || facts["request_digest"] != facts["prepare_request_digest"]
        || facts["activation_command_id"] != facts["activate_command_id"]
        || facts["activation_request_digest"] != facts["activate_request_digest"]
        || !auction_application::valid_external_positive_id(&mid)
        || facts["payment_external_reference"] != format!("mooglemail:main:send:{mid}")
        || !auction_application::valid_mail_player_name(&seller_name)
    {
        return Err(FundingError::Unproven);
    }
    let buyer = amount(&facts, "buyer_amount")?;
    let compensation = amount(&facts, "compensation_amount")?;
    let seller = amount(&facts, "seller_amount")?;
    let tax = amount(&facts, "tax_amount")?;
    FundingSplit {
        buyer: SettlementAmount::new(buyer).map_err(invalid)?,
        compensation: SettlementAmount::new(compensation).map_err(invalid)?,
        seller: SettlementAmount::new(seller).map_err(invalid)?,
        tax: SettlementAmount::new(tax).map_err(invalid)?,
    }
    .validate()
    .map_err(invalid)?;
    if buyer <= 0
        || seller <= 0
        || amount(&facts, "amount")? != seller
        || amount(&facts, "payment_amount")? != buyer
    {
        return Err(FundingError::Unproven);
    }
    let digest = Sha256::digest(
        serde_json::to_vec(&serde_json::json!({"uid":uid,"facts":facts})).map_err(invalid)?,
    )
    .into();
    let preview = LegacyBotSellerPreview {
        obligation_id: obligation,
        lot_id: id(&facts, "lot_id")?,
        payment_obligation_id: payment,
        payment_delivery_id: id(&facts, "payment_delivery_id")?,
        payment_external_mail_id: mid,
        payment_recipient: text(&facts, "payment_recipient")?,
        payment_subject: canonical_settlement_fulfillment_subject(
            kind,
            SettlementId::from_uuid(payment),
        )
        .ok_or(FundingError::Invalid)?,
        state_version: amount(&facts, "state_version")?,
        account_version: amount(&facts, "account_version")?,
        account_status: text(&facts, "account_status")?,
        accounting_mode: super::super::policy::mode(&text(&facts, "accounting_mode")?)?,
        policy_version: amount(&facts, "policy_version")?,
        buyer_amount: buyer,
        compensation_amount: compensation,
        seller_amount: seller,
        tax_amount: tax,
        quarantine_required: seller.checked_add(tax).ok_or(FundingError::Invalid)?,
        digest,
    };
    Ok(Target { preview, facts, seller_name })
}

pub(super) fn text(facts: &Value, key: &str) -> Result<String, FundingError> {
    facts[key].as_str().map(str::to_owned).ok_or(FundingError::Unproven)
}
pub(super) fn id(facts: &Value, key: &str) -> Result<Uuid, FundingError> {
    Uuid::parse_str(&text(facts, key)?).map_err(invalid)
}
fn amount(facts: &Value, key: &str) -> Result<i64, FundingError> {
    facts[key].as_i64().filter(|value| *value >= 0).ok_or(FundingError::Invalid)
}
