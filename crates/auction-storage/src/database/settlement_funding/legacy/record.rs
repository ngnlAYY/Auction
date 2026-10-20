//! 保留原collected历史，追加一次隔离转移、内部留存和审计事实。

use auction_application::FundingMutation;
use serde_json::json;
use sqlx::{Postgres, Transaction};

use super::{super::*, *};

pub(super) async fn retain(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    mutation: &FundingMutation,
    command: &LegacyBotSellerCommand,
    target: &super::preview::Target,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<LegacyBotSellerResult, FundingError> {
    let p = &target.preview;
    sqlx::query("INSERT INTO public.settlement_funding_lots(lot_id,uid,payment_obligation_id,source_kind) VALUES($1,$2,$3,'collected')")
        .bind(p.lot_id).bind(account.uid()).bind(p.payment_obligation_id).execute(&mut **tx).await.map_err(storage)?;
    let quarantine = ledger::allocation(tx, account.uid(), "quarantine", None, None).await?;
    let seller =
        ledger::allocation(tx, account.uid(), "seller", Some(p.lot_id), Some(p.obligation_id))
            .await?;
    let tax = ledger::allocation(tx, account.uid(), "tax", Some(p.lot_id), None).await?;
    let retained = ledger::allocation(tx, account.uid(), "retained", Some(p.lot_id), None).await?;
    // B+C=S+T已经在原冻结快照核验；原收入已在quarantine内，不调用source/issue。
    ledger::transfer(tx, account.uid(), mutation, quarantine, seller, p.quarantine_required)
        .await?;
    ledger::transfer(tx, account.uid(), mutation, seller, tax, p.tax_amount).await?;
    ledger::transfer(tx, account.uid(), mutation, seller, retained, p.seller_amount).await?;
    sqlx::query("UPDATE public.settlement_funding_lots SET disposition='retained' WHERE lot_id=$1")
        .bind(p.lot_id)
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
    let changed=sqlx::query("UPDATE public.settlement_obligation_fulfillments SET execution_state='satisfied',state_version=state_version+1,
        claim_token=NULL,claimed_at=NULL,claim_expires_at=NULL,next_attempt_at=NULL,last_error_code=NULL,
        completed_at=clock_timestamp(),updated_at=clock_timestamp(),proof_kind='bot_legacy_collected_retained_v1',
        proof_reference=$3,proof_digest=$4,proof_observed_at=$5
        WHERE obligation_id=$1 AND execution_state='pending' AND state_version=$2
          AND (claim_token IS NULL OR claim_expires_at<=clock_timestamp())
          AND delivery_id IS NULL AND prepare_command_id IS NULL AND activate_command_id IS NULL AND proof_kind IS NULL")
        .bind(p.obligation_id).bind(p.state_version).bind(command.command_id.to_string()).bind(evidence.digest.as_slice())
        .bind(evidence.observed_from).execute(&mut **tx).await.map_err(storage)?;
    if changed.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    ledger::complete(tx, account, mutation, "legacy_bot_seller", p.obligation_id).await?;
    let result = LegacyBotSellerResult {
        command_id: command.command_id,
        obligation_id: p.obligation_id,
        lot_id: p.lot_id,
        uid: account.uid().to_owned(),
        retained_credits: p.seller_amount,
        tax_credits: p.tax_amount,
    };
    let sent = evidence.sent_collection.as_ref().ok_or(FundingError::Unproven)?;
    let cash = evidence.main_cash.as_ref();
    let mut proof = json!({"credential_id":evidence.identity.credential_id,"credential_version":evidence.identity.credential_version,
        "runtime_generation":evidence.identity.runtime.generation,"uid":evidence.identity.authenticated_uid,
        "player_name":evidence.identity.player_name,"protocol":evidence.identity.protocol_version,
        "binding_digest":evidence.identity.digest,"observed_from":evidence.observed_from,"expires_at":evidence.expires_at,
        "cash_credits":cash.map(|cash|cash.credits),"cash_observed_at":cash.map(|cash|cash.observed_at),"cash_digest":cash.map(|cash|cash.digest),
        "external_mail_id":sent.external_mail_id,"recipient":sent.recipient,"subject":sent.subject,"sent_at":sent.sent_at,
        "collected":sent.collected,"remaining_credits":sent.remaining_credits,"cod_pending":sent.cod_pending,
        "remaining_equipment_ids":sent.remaining_equipment_ids,"sent_observed_at":sent.observed_at,"sent_digest":sent.digest});
    proof["full_snapshot"] = evidence_summary(evidence);
    sqlx::query("INSERT INTO public.settlement_legacy_bot_seller_claims(command_id,lot_id,obligation_id,payment_obligation_id,
        payment_delivery_id,uid,external_mail_id,request_digest,snapshot_digest,evidence_digest,evidence,original_snapshot,result,actor_id)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)")
        .bind(command.command_id).bind(p.lot_id).bind(p.obligation_id).bind(p.payment_obligation_id).bind(p.payment_delivery_id)
        .bind(account.uid()).bind(&p.payment_external_mail_id).bind(mutation.request_digest.as_slice()).bind(p.digest.as_slice())
        .bind(evidence.digest.as_slice()).bind(proof).bind(&target.facts).bind(serde_json::to_value(&result).map_err(invalid)?)
        .bind(&command.actor).execute(&mut **tx).await.map_err(storage)?;
    // 审计来源使用既有角色闭集；具体执行器单独保留在 metadata，不能作为新 source 写入。
    sqlx::query("INSERT INTO public.audit_records(id,occurred_at,actor_type,actor_id,action,entity_type,entity_id,result,source,metadata)
        VALUES($1,clock_timestamp(),'admin',$2,'auction.fulfillment.legacy_collected_retained','settlement_obligation',$3,'success','admin',$4)")
        .bind(command.command_id).bind(&command.actor).bind(p.obligation_id.to_string())
        .bind(json!({"command_id":command.command_id,"lot_id":p.lot_id,"payment_obligation_id":p.payment_obligation_id,
            "payment_delivery_id":p.payment_delivery_id,"uid":account.uid(),"external_mail_id":p.payment_external_mail_id,
            "retained_credits":p.seller_amount,"tax_credits":p.tax_amount,"snapshot_digest":p.digest,"evidence_digest":evidence.digest,
            "executor":"auctionctl"}))
        .execute(&mut **tx).await.map_err(storage)?;
    crate::validate_mail_evidence_time(tx, evidence).await.map_err(evidence_error)?;
    Ok(result)
}

/// 仅保存可重算摘要的安全事实；不让审计JSON成为可反序列化的可信运行时证据。
pub(super) fn evidence_summary(e: &SettlementInternalEvidenceSnapshot) -> serde_json::Value {
    let binding = |b: &auction_application::MailAccountEvidenceBinding| {
        json!({"credential_id":b.credential_id,"credential_version":b.credential_version,
            "runtime":{"generation":b.runtime.generation},"authenticated_uid":b.authenticated_uid,
            "player_name":b.player_name,"world":b.world,"observed_at":b.observed_at,
            "protocol_version":b.protocol_version,"digest":b.digest})
    };
    let purpose = match &e.purpose {
        auction_application::InternalEvidencePurpose::LegacyBotSeller {
            command_id,
            obligation_id,
            expected_state_version,
        } => json!({"LegacyBotSeller":{"command_id":command_id,"obligation_id":obligation_id,
            "expected_state_version":expected_state_version}}),
        _ => serde_json::Value::Null,
    };
    json!({"operation_id":e.operation_id,"purpose":purpose,"identity":binding(&e.identity),
        "equipment":null,"main_cash":e.main_cash.as_ref().map(|c|json!({"binding":binding(&c.binding),
            "credits":c.credits,"observed_at":c.observed_at,"digest":c.digest})),
        "sent_collection":e.sent_collection.as_ref().map(|s|json!({"binding":binding(&s.binding),
            "external_mail_id":s.external_mail_id,"recipient":s.recipient,"subject":s.subject,
            "sent_at":s.sent_at,"observed_at":s.observed_at,"remaining_credits":s.remaining_credits,
            "cod_pending":s.cod_pending,"remaining_equipment_ids":s.remaining_equipment_ids,
            "collected":s.collected,"digest":s.digest})),
        "observed_from":e.observed_from,"expires_at":e.expires_at,"digest":e.digest})
}
