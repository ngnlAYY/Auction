//! 精确delivery锁验、持久sending与追加现金证据审计。

use auction_application::{
    DeliveryPreparedExpectation, DeliveryWorkClaim, SettlementInternalEvidenceSnapshot,
};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::super::{DeliveryAudit, START_SENDING, append_delivery_audit, worker_state_metadata};
use super::transaction::FenceError;

/// 锁等待后再检查实时lease与全部冻结payload/命令，不相信调用方可改写claim。
pub(super) async fn lock_exact_claim(
    tx: &mut Transaction<'_, Postgres>,
    claim: &DeliveryWorkClaim,
) -> Result<(), FenceError> {
    let row = sqlx::query("SELECT command_id,request_digest,origin_kind,origin_id,recipient,payload FROM public.deliveries WHERE id=$1 FOR UPDATE")
        .bind(claim.delivery_id).fetch_optional(&mut **tx).await?.ok_or(FenceError::Rejected)?;
    if row.try_get::<Uuid, _>("command_id")? != claim.command_id
        || row.try_get::<Vec<u8>, _>("request_digest")? != claim.request_hash.as_bytes()
        || row.try_get::<String, _>("origin_kind")? != claim.origin_kind
        || row.try_get::<String, _>("origin_id")? != claim.origin_id
        || row.try_get::<String, _>("recipient")? != claim.recipient
        || row.try_get::<serde_json::Value, _>("payload")? != claim.payload
    {
        return Err(FenceError::Rejected);
    }
    let live: bool = sqlx::query_scalar("SELECT status='claimed' AND lease_token=$2 AND fencing_version=$3 AND lease_expires_at>clock_timestamp() FROM public.deliveries WHERE id=$1")
        .bind(claim.delivery_id).bind(claim.lease_token).bind(claim.fencing_version).fetch_one(&mut **tx).await?;
    if !live {
        return Err(FenceError::Rejected);
    }
    Ok(())
}

pub(super) async fn persist_sending(
    tx: &mut Transaction<'_, Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
    evidence: Option<&SettlementInternalEvidenceSnapshot>,
    expectation: Option<&DeliveryPreparedExpectation>,
) -> Result<(), FenceError> {
    let changed = sqlx::query(concat!(
        "UPDATE public.deliveries AS deliveries SET status='sending',sending_at=$2,updated_at=clock_timestamp() WHERE id=$1 AND status='claimed' AND lease_token=$3 AND fencing_version=$4 AND lease_expires_at>$2 AND lease_expires_at>clock_timestamp() AND ",
        delivery_completion_gate!()
    )).bind(claim.delivery_id).bind(started_at).bind(claim.lease_token).bind(claim.fencing_version)
        .execute(&mut **tx).await?;
    if changed.rows_affected() != 1 {
        return Err(FenceError::Rejected);
    }
    let request_id = claim.command_id.to_string();
    let mut metadata =
        worker_state_metadata("sending", claim.fencing_version, claim.attempt_count, None);
    match expectation {
        Some(DeliveryPreparedExpectation::Send(expectation)) => {
            super::super::attempts::freeze(tx, claim, started_at, expectation).await?;
            metadata["send_expectation"] =
                serde_json::to_value(expectation.as_ref()).map_err(|_| FenceError::Rejected)?;
        }
        Some(DeliveryPreparedExpectation::Take(expectation)) => {
            super::super::take_attempts::freeze(tx, claim, started_at, expectation).await?;
            metadata["take_expectation"] =
                serde_json::to_value(expectation.as_ref()).map_err(|_| FenceError::Rejected)?;
        }
        None => {}
    }
    if let Some(evidence) = evidence {
        // 追加非敏感瞬时证明，不把generation/观测时间混入稳定reserve命令摘要。
        metadata["cash_authorization"] = serde_json::json!({
            "operation_id": evidence.operation_id,
            "request_digest": claim.request_hash.as_bytes(),
            "snapshot_digest": evidence.digest,
            "observed_from": evidence.observed_from,
            "expires_at": evidence.expires_at,
            "identity": binding_metadata(&evidence.identity),
            "cash": evidence.main_cash.as_ref().map(|cash|serde_json::json!({"binding": binding_metadata(&cash.binding), "credits": cash.credits,
                "observed_at": cash.observed_at, "digest": cash.digest})),
            "accounting_mode": if evidence.main_cash.is_none() {"record_only"} else {"strict_cash"},
        });
    }
    append_delivery_audit(
        tx,
        DeliveryAudit {
            occurred_at: started_at,
            actor_type: "worker",
            actor_id: "delivery-worker",
            action: START_SENDING,
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "worker",
            metadata,
        },
    )
    .await?;
    Ok(())
}

/// 只保存规范化身份事实；Cookie、页面令牌和响应正文不进入审计。
fn binding_metadata(
    binding: &auction_application::MailAccountEvidenceBinding,
) -> serde_json::Value {
    serde_json::json!({
        "credential_id": binding.credential_id, "credential_version": binding.credential_version,
        "runtime_scope": auction_application::MAIL_RUNTIME_SCOPE, "generation": binding.runtime.generation,
        "uid": binding.authenticated_uid, "player_name": binding.player_name,
        "world": binding.world.as_str(),
        "observed_at": binding.observed_at, "protocol_version": binding.protocol_version,
        "digest": binding.digest,
    })
}
