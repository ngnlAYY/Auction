//! Serializable 三次整事务重试；只有确认回滚才允许释放 opaque commit guard。

use super::{conflicts, evidence_error, locks, persistence, sql_error, validation};
use crate::settlement_funding;
use auction_application::{
    FundingCollectedReceipt, FundingError, FundingMutation, SettlementFulfillmentProof,
    SettlementInternalCommand, SettlementInternalCommitOutcome as Outcome,
    SettlementInternalEvidenceSnapshot,
};
use auction_domain::InternalSettlementAction;
use sqlx::{Postgres, Transaction};

pub(super) async fn run(
    pool: &crate::database::TrackedPool<Postgres>,
    command: &SettlementInternalCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
    proof: Option<&SettlementFulfillmentProof>,
    external_refund: bool,
) -> Result<Outcome, FundingError> {
    for attempt in 0..3 {
        let mut tx = pool.begin().await.map_err(sql_error)?;
        let operation = async {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                .execute(&mut *tx)
                .await
                .map_err(sql_error)?;
            persistence::validate_command(
                command,
                if external_refund {
                    "external_refund"
                } else if proof.is_some() {
                    "collected"
                } else {
                    "internal"
                },
            )?;
            if persistence::replay(&mut tx, command).await? {
                return Ok(false);
            }
            // PostgreSQL 18 的整事务超时覆盖锁等待与commit，不逐语句重置预算。
            let budget = (evidence.expires_at.min(command.claim.lease_until) - chrono::Utc::now())
                .num_milliseconds();
            if budget <= 0 {
                return Err(FundingError::Unproven);
            }
            sqlx::query("SELECT set_config('transaction_timeout',$1,true)")
                .bind(format!("{budget}ms"))
                .execute(&mut *tx)
                .await
                .map_err(sql_error)?;
            apply(&mut tx, command, evidence, proof, external_refund).await
        }
        .await;
        match operation {
            Ok(frozen) => match tx.commit().await {
                Ok(()) => {
                    return Ok(if frozen { Outcome::FundingDeferred } else { Outcome::Committed });
                }
                Err(error) => match sql_error(error) {
                    FundingError::SerializationConflict if attempt < 2 => continue,
                    FundingError::SerializationConflict => {
                        return Ok(Outcome::RolledBack(FundingError::SerializationConflict));
                    }
                    error => return Err(error),
                },
            },
            Err(error) => {
                tx.rollback().await.map_err(sql_error)?;
                if matches!(error, FundingError::SerializationConflict) && attempt < 2 {
                    continue;
                }
                return Ok(Outcome::RolledBack(error));
            }
        }
    }
    Ok(Outcome::RolledBack(FundingError::SerializationConflict))
}

async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    command: &SettlementInternalCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
    proof: Option<&SettlementFulfillmentProof>,
    external_refund: bool,
) -> Result<bool, FundingError> {
    crate::lock_mail_runtime_generation(tx, &evidence.identity.runtime)
        .await
        .map_err(evidence_error)?;
    locks::worlds(tx).await?;
    let account =
        settlement_funding::lock_funding_account(tx, &evidence.identity.authenticated_uid).await?;
    // 有其他受保护账号时，不允许把错账号降级为legacy。
    let protected = settlement_funding::funding_requires_authorization(tx).await?;
    locks::before_credentials(tx, &command.claim).await?;
    crate::lock_mail_evidence_credentials(tx, evidence).await.map_err(evidence_error)?;
    locks::after_credentials(tx, &command.claim).await?;
    validation::validate(tx, command, evidence, proof.is_some(), external_refund).await?;
    if proof.is_none()
        && !external_refund
        && crate::database::settlement_legacy_repair::legacy_self_uid(
            tx,
            command.claim.obligation_id.as_uuid(),
        )
        .await?
        .is_some_and(|uid| uid != evidence.identity.authenticated_uid)
    {
        return Err(FundingError::Unproven);
    }
    super::graph::validate_graph(tx, &command.claim).await?;
    let mutation =
        FundingMutation { command_id: command.command_id, request_digest: command.request_digest };
    if let Some(proof) = proof {
        validate_collected(tx, command, evidence, proof).await?;
        if account.as_ref().is_some_and(|a| !a.ready()) || (account.is_none() && protected) {
            return Err(FundingError::Unavailable);
        }
        if let Some(account) = &account {
            // 同一持久策略决定现金或纯记账证明；缺口只提交冻结，不满足付款或解除依赖。
            if !settlement_funding::check_funding_evidence(tx, account, evidence).await? {
                crate::validate_mail_evidence_time(tx, evidence).await.map_err(evidence_error)?;
                return Ok(true);
            }
        }
        // 前置现金门通过后才写原满足事实；发行后的覆盖校验失败仍整事务回滚。
        persistence::satisfy(tx, command, evidence, Some(proof)).await?;
        if let Some(account) = &account {
            let receipt = FundingCollectedReceipt {
                payment_obligation_id: command.claim.obligation_id.as_uuid(),
                delivery_id: command.claim.delivery_id.ok_or(FundingError::Unproven)?,
                buyer_credits: command.claim.amount,
                evidence,
            };
            settlement_funding::fund_collected_payment(tx, account, &mutation, &receipt).await?;
        }
        // 未建立账户时保持原外部满足语义；命令仅记录观察，不生成可消费allocation。
    } else {
        let action = InternalSettlementAction::for_kind(command.claim.kind)
            .ok_or(FundingError::Unsupported)?;
        conflicts::exclude_asset_conflicts(tx, &command.claim).await?;
        if external_refund && account.is_none() && !protected {
            // 无资金账户的旧外部退款继续原路径，不创建资金授权事实。
            return Ok(false);
        }
        if action.requires_funding() {
            let account =
                account.as_ref().filter(|a| a.ready()).ok_or(FundingError::Unavailable)?;
            if !settlement_funding::check_funding_evidence(tx, account, evidence).await? {
                crate::validate_mail_evidence_time(tx, evidence).await.map_err(evidence_error)?;
                return Ok(true);
            }
            match action {
                InternalSettlementAction::Purchase => {
                    settlement_funding::fund_internal_purchase(
                        tx,
                        account,
                        &mutation,
                        command.claim.lot_id.as_uuid(),
                    )
                    .await?
                }
                InternalSettlementAction::RetainSeller => {
                    settlement_funding::retain_funded_seller(
                        tx,
                        account,
                        &mutation,
                        command.claim.obligation_id.as_uuid(),
                    )
                    .await?
                }
                InternalSettlementAction::Refund => {
                    settlement_funding::refund_funded_purchase(
                        tx,
                        account,
                        &mutation,
                        command.claim.obligation_id.as_uuid(),
                        !external_refund,
                    )
                    .await?
                }
                InternalSettlementAction::KeepUnsold | InternalSettlementAction::Handoff => {
                    return Err(FundingError::Invalid);
                }
            }
        }
        if !external_refund {
            persistence::satisfy(tx, command, evidence, None).await?;
        }
    }
    persistence::complete(tx, command, evidence, proof.is_some(), external_refund).await?;
    Ok(false)
}

async fn validate_collected(
    tx: &mut Transaction<'_, Postgres>,
    command: &SettlementInternalCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
    proof: &SettlementFulfillmentProof,
) -> Result<(), FundingError> {
    let c = &command.claim;
    let sent = evidence.sent_collection.as_ref().ok_or(FundingError::Unproven)?;
    let delivery = c.delivery_snapshot.as_ref().ok_or(FundingError::Unproven)?;
    if proof.kind != "mooglemail_sent_collected"
        || proof.digest == [0; 32]
        || proof.reference != sent.external_mail_id
        || sent.recipient != c.recipient
        || sent.binding.world != c.world
        || !matches!(
            c.kind,
            auction_domain::SettlementObligationKind::WinnerEquipmentCod
                | auction_domain::SettlementObligationKind::WinnerPaymentCarrierCod
        )
        || proof.observed_at < c.claimed_at
        || proof.observed_at > chrono::Utc::now()
        || auction_application::canonical_settlement_fulfillment_subject(c.kind, c.obligation_id)
            .as_deref()
            != Some(sent.subject.as_str())
    {
        return Err(FundingError::Unproven);
    }
    let proven: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.deliveries d JOIN public.settlement_obligation_fulfillments f ON f.delivery_id=d.id
         WHERE d.id=$1 AND f.obligation_id=$2 AND d.status='succeeded' AND d.origin_kind='settlement'
          AND d.origin_id=$2::uuid::text AND d.recipient=$3 AND d.payload=$4
          AND d.command_id=$5 AND d.request_digest=$6
          AND d.activation_command_id=$7 AND d.activation_request_digest=$8
          AND d.external_reference=$9
          AND (d.result_metadata->>'proof' IN ('sent_exact_payload_v1','sent_recovered_reference_v1')
            OR public.delivery_has_frozen_sent_proof(d.id,d.result_metadata,d.external_reference))
          AND d.result_metadata->>'external_reference_proven'='true'
          AND d.result_metadata->>'operation'='send')"
    ).bind(delivery.id).bind(c.obligation_id.as_uuid()).bind(&c.recipient).bind(&delivery.payload)
        .bind(c.prepare_command_id.map(|id| id.as_uuid())).bind(c.prepare_request_hash.map(|h| h.as_bytes().to_vec()))
        .bind(c.activate_command_id.map(|id| id.as_uuid())).bind(c.activate_request_hash.map(|h| h.as_bytes().to_vec()))
        .bind(format!("mooglemail:{}:send:{}",validation::world(c.world),sent.external_mail_id))
        .fetch_one(&mut **tx).await.map_err(sql_error)?;
    if !proven {
        return Err(FundingError::Unproven);
    }
    Ok(())
}
