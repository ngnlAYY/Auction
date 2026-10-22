//! 独立命令账本、不可变证明、满足与直接blocked依赖同事务提交。

use super::sql_error;
use auction_application::{
    FundingError, SettlementFulfillmentProof, SettlementInternalCommand,
    SettlementInternalEvidenceSnapshot,
};
use sqlx::{Postgres, Row, Transaction};

// 保留既有 v1 命令摘要。新记录冻结原地址；旧记录仅从同 UUID 的改名历史重建，
// 每个候选仍须重建完整原 payload 并逐字节命中既有摘要，绝不只比较义务 ID。
const REPLAY_QUERY: &str = "SELECT obligation_id,request_digest,action,
       ARRAY(SELECT recipient FROM (
         SELECT c.evidence->>'recipient' AS recipient
         UNION SELECT previous_username FROM public.user_renames WHERE user_id=$2
         UNION SELECT next_username FROM public.user_renames WHERE user_id=$2
         UNION SELECT previous_username FROM public.user_game_identity_history WHERE user_id=$2
         UNION SELECT next_username FROM public.user_game_identity_history WHERE user_id=$2
       ) names WHERE recipient IS NOT NULL) AS replay_recipients
     FROM public.settlement_internal_commands c WHERE command_id=$1";

pub(super) async fn replay_pool(
    pool: &crate::database::TrackedPool<Postgres>,
    c: &SettlementInternalCommand,
) -> Result<bool, FundingError> {
    let row = sqlx::query(REPLAY_QUERY)
        .bind(c.command_id)
        .bind(c.claim.recipient_user_id)
        .fetch_optional(pool)
        .await
        .map_err(sql_error)?;
    matches_replay(row.as_ref(), c)
}

pub(super) async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    c: &SettlementInternalCommand,
) -> Result<bool, FundingError> {
    let row = sqlx::query(REPLAY_QUERY)
        .bind(c.command_id)
        .bind(c.claim.recipient_user_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(sql_error)?;
    matches_replay(row.as_ref(), c)
}

fn matches_replay(
    row: Option<&sqlx::postgres::PgRow>,
    c: &SettlementInternalCommand,
) -> Result<bool, FundingError> {
    let Some(row) = row else { return Ok(false) };
    let action = row.try_get::<String, _>("action").map_err(sql_error)?;
    validate_command(c, &action)?;
    if row.try_get::<uuid::Uuid, _>("obligation_id").map_err(sql_error)?
        != c.claim.obligation_id.as_uuid()
    {
        return Err(FundingError::Conflict);
    }
    let stored_digest = row.try_get::<Vec<u8>, _>("request_digest").map_err(sql_error)?;
    if stored_digest == c.request_digest {
        return Ok(true);
    }
    if action == "external_refund" && c.claim.recipient_user_id.is_some() {
        for recipient in row.try_get::<Vec<String>, _>("replay_recipients").map_err(sql_error)? {
            let mut original = c.claim.clone();
            original.recipient = recipient;
            let expected =
                SettlementInternalCommand::external_refund(original, c.recipient_version)
                    .map_err(|_| FundingError::Invalid)?;
            if expected.command_id == c.command_id && stored_digest == expected.request_digest {
                return Ok(true);
            }
        }
    }
    Err(FundingError::Conflict)
}

pub(super) fn validate_command(
    c: &SettlementInternalCommand,
    action: &str,
) -> Result<(), FundingError> {
    let expected = match action {
        "internal" => SettlementInternalCommand::new(c.claim.clone(), c.recipient_version, false),
        "collected" => SettlementInternalCommand::new(c.claim.clone(), c.recipient_version, true),
        "external_refund" => {
            SettlementInternalCommand::external_refund(c.claim.clone(), c.recipient_version)
        }
        _ => return Err(FundingError::Invalid),
    }
    .map_err(|_| FundingError::Invalid)?;
    if expected.command_id != c.command_id || expected.request_digest != c.request_digest {
        return Err(FundingError::Conflict);
    }
    Ok(())
}

pub(super) async fn satisfy(
    tx: &mut Transaction<'_, Postgres>,
    c: &SettlementInternalCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
    external: Option<&SettlementFulfillmentProof>,
) -> Result<(), FundingError> {
    let kind = external.map_or("bot_internal_v1", |p| p.kind.as_str());
    let reference = external.map_or_else(|| c.command_id.to_string(), |p| p.reference.clone());
    let digest = external.map_or(evidence.digest, |p| p.digest);
    let observed = external.map_or(evidence.observed_from, |p| p.observed_at);
    let updated = sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments SET execution_state='satisfied',
         proof_kind=$2,proof_reference=$3,proof_digest=$4,proof_observed_at=$5,
         claim_token=NULL,claimed_at=NULL,claim_expires_at=NULL,
         completed_at=clock_timestamp(),updated_at=clock_timestamp(),state_version=state_version+1,
         next_attempt_at=NULL,last_error_code=NULL
         WHERE obligation_id=$1 AND execution_state=$6 AND state_version=$7
           AND claim_token=$8 AND claim_fencing=$9 AND claim_expires_at>clock_timestamp()",
    )
    .bind(c.claim.obligation_id.as_uuid())
    .bind(kind)
    .bind(reference)
    .bind(digest.to_vec())
    .bind(observed)
    .bind(if external.is_some() { "enqueued" } else { "pending" })
    .bind(i64::try_from(c.claim.state_version).map_err(|_| FundingError::Invalid)?)
    .bind(c.claim.claim_token)
    .bind(c.claim.claim_fencing)
    .execute(&mut **tx)
    .await
    .map_err(sql_error)?;
    if updated.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    Ok(())
}

pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    c: &SettlementInternalCommand,
    e: &SettlementInternalEvidenceSnapshot,
    collected: bool,
    external_refund: bool,
) -> Result<(), FundingError> {
    crate::validate_mail_evidence_time(tx, e).await.map_err(super::evidence_error)?;
    let lease_valid: bool = sqlx::query_scalar("SELECT $1::timestamptz>clock_timestamp()")
        .bind(c.claim.lease_until)
        .fetch_one(&mut **tx)
        .await
        .map_err(sql_error)?;
    if !lease_valid {
        return Err(FundingError::Conflict);
    }
    let released = if external_refund {
        0
    } else {
        sqlx::query("UPDATE public.settlement_obligations SET business_state='ready',state_version=state_version+1,updated_at=clock_timestamp() WHERE prerequisite_obligation_id=$1 AND business_state='blocked'")
            .bind(c.claim.obligation_id.as_uuid()).execute(&mut **tx).await.map_err(sql_error)?.rows_affected()
    };
    let binding = |b: &auction_application::MailAccountEvidenceBinding| {
        serde_json::json!({
            "credential_id":b.credential_id,"credential_version":b.credential_version,
            "generation":b.runtime.generation,"uid":b.authenticated_uid,"player_name":b.player_name,
            "world":b.world,"observed_at":b.observed_at,"protocol":b.protocol_version,"digest":b.digest
        })
    };
    let evidence = serde_json::json!({
        "operation_id":e.operation_id,"identity":binding(&e.identity),
        "recipient":c.claim.recipient,"recipient_user_id":c.claim.recipient_user_id,
        "claim_token":c.claim.claim_token,"claim_fencing":c.claim.claim_fencing,"state_version":c.claim.state_version,
        "equipment":e.equipment.as_ref().map(|v| serde_json::json!({
            "binding":binding(&v.binding),"equipment_id":v.equipment_id,"external_equipment_id":v.external_equipment_id,
            "category":v.category,"observed_at":v.observed_at,"digest":v.digest
        })),
        "cash":e.main_cash.as_ref().map(|v| serde_json::json!({"binding":binding(&v.binding),"credits":v.credits,"observed_at":v.observed_at,"digest":v.digest})),
        "sent":e.sent_collection.as_ref().map(|v| serde_json::json!({
            "binding":binding(&v.binding),"external_mail_id":v.external_mail_id,"recipient":v.recipient,
            "subject":v.subject,"sent_at":v.sent_at,"observed_at":v.observed_at,"remaining_credits":v.remaining_credits,
            "cod_pending":v.cod_pending,"equipment":v.remaining_equipment_ids,"collected":v.collected,"digest":v.digest
        })),
        "observed_from":e.observed_from,"expires_at":e.expires_at,"digest":e.digest
    });
    sqlx::query("INSERT INTO public.settlement_internal_commands(command_id,obligation_id,action,request_digest,uid,evidence_digest,evidence) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(c.command_id).bind(c.claim.obligation_id.as_uuid()).bind(if external_refund { "external_refund" } else if collected { "collected" } else { "internal" })
        .bind(c.request_digest.to_vec()).bind(&e.identity.authenticated_uid).bind(e.digest.to_vec()).bind(evidence)
        .execute(&mut **tx).await.map_err(sql_error)?;
    sqlx::query("INSERT INTO public.audit_records(id,occurred_at,actor_type,actor_id,action,entity_type,entity_id,result,source,metadata) VALUES($1,clock_timestamp(),'system','auction-fulfillment',$2,'settlement_obligation',$3,'success','worker',$4)")
        .bind(c.command_id).bind(if external_refund { "auction.fulfillment.refund_funded" } else if collected { "auction.fulfillment.collected_funded" } else { "auction.fulfillment.internally_satisfied" })
        .bind(c.claim.obligation_id.to_string()).bind(serde_json::json!({"command_id":c.command_id,"uid":e.identity.authenticated_uid,"evidence_digest":e.digest,"activated_dependent_count":released}))
        .execute(&mut **tx).await.map_err(sql_error)?;
    Ok(())
}
