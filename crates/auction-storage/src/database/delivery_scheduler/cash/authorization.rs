//! Credits Send资金授权；所有远端观察已在持同session的业务事务外完成。

use auction_application::{
    DeliveryFenceOutcome, DeliveryPreparedExpectation, DeliveryWorkClaim, FundingMutation,
    InternalEvidencePurpose, SettlementInternalEvidenceSnapshot,
};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    persistence::{lock_exact_claim, persist_sending},
    transaction::FenceError,
};
use crate::database::settlement_funding as funding;

/// 无证据入口在任何业务行锁之前检查保护；None不等于发送授权。
pub(super) async fn authorize_legacy(
    tx: &mut Transaction<'_, Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
    expectation: Option<&DeliveryPreparedExpectation>,
) -> Result<DeliveryFenceOutcome, FenceError> {
    let credential = expectation.map(|expectation| match expectation {
        DeliveryPreparedExpectation::Send(expectation) => {
            (expectation.world_id.as_str(), expectation.credential.as_ref())
        }
        DeliveryPreparedExpectation::Take(expectation) => {
            (expectation.world_id.as_str(), expectation.credential.as_ref())
        }
    });
    if let Some((_, Some(binding))) = credential {
        crate::lock_mail_runtime_generation(
            tx,
            &auction_application::MailRuntimeBinding { generation: binding.runtime_generation },
        )
        .await?;
    }
    let account = funding::lock_funded_delivery_account(tx, claim.delivery_id).await?;
    let protected = funding::funding_requires_authorization(tx).await?;
    if account.is_some() || (requires_cash_gate(claim) && protected) {
        return Err(FenceError::Rejected);
    }
    crate::database::lot_watch::lock_delivery_lot(tx, claim.delivery_id).await?;
    if let Some((world_id, binding)) = credential
        && !super::super::attempts::lock_credential_binding(tx, world_id, binding).await?
    {
        return Err(FenceError::Rejected);
    }
    lock_exact_claim(tx, claim).await?;
    persist_sending(tx, claim, started_at, None, expectation).await?;
    Ok(DeliveryFenceOutcome::Sending)
}

fn requires_cash_gate(claim: &DeliveryWorkClaim) -> bool {
    claim.payload.get("operation").and_then(serde_json::Value::as_str) == Some("send")
        && claim.payload.get("credits").and_then(serde_json::Value::as_i64) != Some(0)
}

pub(super) async fn authorize(
    tx: &mut Transaction<'_, Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
    evidence: &SettlementInternalEvidenceSnapshot,
    expectation: Option<&DeliveryPreparedExpectation>,
) -> Result<DeliveryFenceOutcome, FenceError> {
    if !requires_cash_gate(claim)
        || evidence.operation_id != claim.command_id
        || evidence.purpose
            != (InternalEvidencePurpose::DeliveryClaim {
                delivery_id: claim.delivery_id,
                claim_token: claim.lease_token,
                claim_fencing: claim.fencing_version,
                request_hash: claim.request_hash,
            })
    {
        return Err(FenceError::Rejected);
    }
    let binding = expectation
        .and_then(|expectation| match expectation {
            DeliveryPreparedExpectation::Send(expectation) => expectation.credential.as_ref(),
            DeliveryPreparedExpectation::Take(_) => None,
        })
        .ok_or(FenceError::Rejected)?;
    if binding.credential_id != evidence.identity.credential_id
        || binding.credential_version != evidence.identity.credential_version
        || binding.runtime_generation != evidence.identity.runtime.generation
    {
        return Err(FenceError::Rejected);
    }
    crate::lock_mail_runtime_generation(tx, &evidence.identity.runtime).await?;
    // 持久world code字典序；Main现金与Isekai身份只共享账号，不能搬用世界事实。
    let world = match evidence.identity.world {
        auction_domain::SettlementWorld::Main => "main",
        auction_domain::SettlementWorld::Isekai => "isekai",
    };
    if claim.payload.get("world").and_then(serde_json::Value::as_str) != Some(world) {
        return Err(FenceError::Rejected);
    }
    let worlds = sqlx::query_scalar::<_, String>(
        "SELECT id FROM public.worlds WHERE id IN ('main', $1) ORDER BY id FOR UPDATE",
    )
    .bind(world)
    .fetch_all(&mut **tx)
    .await?;
    if !worlds.iter().any(|value| value == world) || !worlds.iter().any(|value| value == "main") {
        return Err(FenceError::Rejected);
    }
    let bound = funding::lock_funded_delivery_account(tx, claim.delivery_id).await?;
    // 已有allocation必须匹配本次认证UID；拒绝后不得改锁另一账户或走legacy。
    let account = if let Some(bound) = bound {
        if bound.uid() != evidence.identity.authenticated_uid {
            return Err(FenceError::Rejected);
        }
        Some(bound)
    } else {
        funding::lock_funding_account(tx, &evidence.identity.authenticated_uid).await?
    };
    let protected = funding::funding_requires_authorization(tx).await?;
    crate::database::lot_watch::lock_delivery_lot(tx, claim.delivery_id).await?;
    crate::lock_mail_evidence_credentials(tx, evidence).await?;
    // 只锁实际使用的义务/履约事实，不反锁不需要的auction/recovery；冻结金额不可改写。
    let obligation = if claim.origin_kind == "settlement" {
        let id = Uuid::parse_str(&claim.origin_id).map_err(|_| FenceError::Rejected)?;
        let exists = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM public.settlement_obligations WHERE id=$1 AND business_state='ready' FOR UPDATE",
        ).bind(id).fetch_optional(&mut **tx).await?;
        if exists != Some(id) {
            return Err(FenceError::Rejected);
        }
        Some(id)
    } else {
        None
    };
    lock_exact_claim(tx, claim).await?;
    if let Some(id) = obligation {
        let exists = sqlx::query_scalar::<_, Uuid>(
            "SELECT obligation_id FROM public.settlement_obligation_fulfillments
             WHERE obligation_id=$1 AND delivery_id=$2 AND execution_state='enqueued'
               AND prepare_command_id=$3 AND prepare_request_digest=$4
             FOR UPDATE",
        )
        .bind(id)
        .bind(claim.delivery_id)
        .bind(claim.command_id)
        .bind(claim.request_hash.as_bytes().as_slice())
        .fetch_optional(&mut **tx)
        .await?;
        if exists != Some(id) {
            return Err(FenceError::Rejected);
        }
    }
    crate::validate_mail_evidence_time(tx, evidence).await?;
    if let Some(account) = account {
        if !account.ready() {
            return Err(FenceError::Rejected);
        }
        if !funding::check_funding_evidence(tx, &account, evidence).await? {
            // B的账实缺口冻结必须提交；绝不能把该提交当作发送成功。
            return Ok(DeliveryFenceOutcome::Deferred);
        }
        let digest = auction_application::RequestHash::from_request(
            "delivery_cash_reserve_v1",
            claim.delivery_id,
            &(
                account.uid(),
                claim.command_id,
                claim.request_hash.as_bytes(),
                &claim.recipient,
                &claim.payload,
            ),
        )
        .map_err(|_| FenceError::Rejected)?;
        let mutation =
            FundingMutation { command_id: claim.delivery_id, request_digest: digest.as_bytes() };
        funding::reserve_funded_delivery(tx, &account, &mutation, claim.delivery_id).await?;
        funding::start_funded_delivery(tx, &account, claim.delivery_id).await?;
    } else if protected {
        // 有其他受保护账户但无法解释当前账号时失败关闭，不凭缺关联自动legacy。
        return Err(FenceError::Rejected);
    } else if evidence.main_cash.is_none() {
        // 未开户legacy仍按旧strict证据形状，不把缺现金证明本身当成record_only授权。
        return Err(FenceError::Rejected);
    }
    crate::validate_mail_evidence_time(tx, evidence).await?;
    persist_sending(tx, claim, started_at, Some(evidence), expectation).await?;
    Ok(DeliveryFenceOutcome::Sending)
}
