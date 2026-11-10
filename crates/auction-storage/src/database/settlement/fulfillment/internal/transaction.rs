//! Serializable 三次整事务重试；只有确认回滚才允许释放 opaque commit guard。

use super::{conflicts, evidence_error, locks, persistence, sql_error, validation};
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding as settlement_funding;
use auction_application::settlement::{
    FundingCollectedReceipt, FundingError, FundingMutation, SettlementFulfillmentProof,
    SettlementInternalCommand, SettlementInternalCommitOutcome as Outcome,
    SettlementInternalEvidenceSnapshot,
};
use auction_domain::InternalSettlementAction;
use sqlx::Postgres;

#[tracing::instrument(
    target = "auction::settlement::funding_diagnostics",
    level = "warn",
    skip_all,
    fields(
        obligation_id = %command.claim.obligation_id,
        delivery_id = ?command.claim.delivery_id,
        command_id = %command.command_id,
        world = command.claim.world.as_str()
    )
)]
pub(super) async fn run(
    pool: &crate::database::TrackedPool<Postgres>,
    command: &SettlementInternalCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
    proof: Option<&SettlementFulfillmentProof>,
    external_refund: bool,
) -> Result<Outcome, FundingError> {
    pool.observe_transaction(auction_observability::DatabaseTransactionOwner::Fulfillment, async {
        let rejected = |kind, stage, error| diagnose_collected(proof.is_some(), kind, stage, error);
        for attempt in 0..3 {
            let mut tx = pool
                .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
                .await
                .map_err(sql_error)
                .map_err(|error| {
                    rejected("collected_transaction_begin_failed", "transaction_begin", error)
                })?;
            let operation = async {
                persistence::validate_command(
                    command,
                    if external_refund {
                        "external_refund"
                    } else if proof.is_some() {
                        "collected"
                    } else {
                        "internal"
                    },
                )
                .map_err(|error| {
                    rejected("collected_transaction_command_failed", "transaction_command", error)
                })?;
                if persistence::replay(&mut tx, command).await.map_err(|error| {
                    rejected("collected_transaction_replay_failed", "transaction_replay", error)
                })? {
                    return Ok(false);
                }
                // PostgreSQL 18 的整事务超时覆盖锁等待与commit，不逐语句重置预算。
                let budget = (evidence.expires_at.min(command.claim.lease_until)
                    - chrono::Utc::now())
                .num_milliseconds();
                if budget <= 0 {
                    return Err(rejected(
                        "collected_transaction_budget_expired",
                        "transaction_budget",
                        FundingError::Unproven,
                    ));
                }
                sqlx::query("SELECT set_config('transaction_timeout',$1,true)")
                    .bind(format!("{budget}ms"))
                    .execute(&mut *tx)
                    .await
                    .map_err(sql_error)
                    .map_err(|error| {
                        rejected(
                            "collected_transaction_timeout_setup_failed",
                            "transaction_timeout",
                            error,
                        )
                    })?;
                apply(&mut tx, command, evidence, proof, external_refund).await
            }
            .await;
            match operation {
                Ok(frozen) => match tx.commit().await {
                    Ok(()) => {
                        return Ok(if frozen {
                            Outcome::FundingDeferred
                        } else {
                            Outcome::Committed
                        });
                    }
                    Err(error) => match rejected(
                        "collected_transaction_commit_failed",
                        "transaction_commit",
                        sql_error(error),
                    ) {
                        FundingError::SerializationConflict if attempt < 2 => continue,
                        FundingError::SerializationConflict => {
                            auction_observability::database_transaction_exhausted();
                            return Ok(Outcome::RolledBack(FundingError::SerializationConflict));
                        }
                        error => return Err(error),
                    },
                },
                Err(error) => {
                    tx.rollback().await.map_err(sql_error).map_err(|error| {
                        rejected(
                            "collected_transaction_rollback_failed",
                            "transaction_rollback",
                            error,
                        )
                    })?;
                    if matches!(error, FundingError::SerializationConflict) && attempt < 2 {
                        continue;
                    }
                    crate::database::observe_retry_exhaustion(
                        attempt + 1,
                        3,
                        matches!(error, FundingError::SerializationConflict),
                    );
                    return Ok(Outcome::RolledBack(error));
                }
            }
        }
        Ok(Outcome::RolledBack(FundingError::SerializationConflict))
    })
    .await
}

async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    command: &SettlementInternalCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
    proof: Option<&SettlementFulfillmentProof>,
    external_refund: bool,
) -> Result<bool, FundingError> {
    let rejected = |kind, stage, error| diagnose_collected(proof.is_some(), kind, stage, error);
    crate::lock_mail_runtime_generation(tx, &evidence.identity.runtime)
        .await
        .map_err(evidence_error)
        .map_err(|error| {
            rejected("collected_runtime_generation_failed", "runtime_generation", error)
        })?;
    crate::database::settlement::funding::lock_funding_worlds(tx)
        .await
        .map_err(|error| rejected("collected_world_lock_failed", "world_lock", error))?;
    let account =
        settlement_funding::lock_funding_account(tx, &evidence.identity.authenticated_uid)
            .await
            .map_err(|error| rejected("collected_account_lock_failed", "account_lock", error))?;
    // 有其他受保护账号时，不允许把错账号降级为legacy。
    let protected = settlement_funding::funding_requires_authorization(tx)
        .await
        .map_err(|error| rejected("collected_funding_scope_failed", "funding_scope", error))?;
    locks::before_credentials(tx, &command.claim)
        .await
        .map_err(|error| rejected("collected_business_lock_failed", "business_lock", error))?;
    crate::lock_mail_evidence_credentials(tx, evidence)
        .await
        .map_err(evidence_error)
        .map_err(|error| rejected("collected_credentials_failed", "credentials", error))?;
    locks::after_credentials(tx, &command.claim)
        .await
        .map_err(|error| rejected("collected_delivery_lock_failed", "delivery_lock", error))?;
    validation::validate(tx, command, evidence, proof.is_some(), external_refund)
        .await
        .map_err(|error| rejected("collected_claim_validation_failed", "claim_validate", error))?;
    if proof.is_none()
        && !external_refund
        && crate::database::delivery::legacy_self_proof::legacy_self_uid(
            tx,
            command.claim.obligation_id.as_uuid(),
        )
        .await?
        .is_some_and(|uid| uid != evidence.identity.authenticated_uid)
    {
        return Err(FundingError::Unproven);
    }
    super::graph::validate_graph(tx, &command.claim)
        .await
        .map_err(|error| rejected("collected_graph_validation_failed", "graph_validate", error))?;
    let mutation =
        FundingMutation { command_id: command.command_id, request_digest: command.request_digest };
    if let Some(proof) = proof {
        validate_collected(tx, command, evidence, proof).await.map_err(|error| {
            rejected("collected_proof_validation_failed", "collected_proof", error)
        })?;
        if account.as_ref().is_some_and(|a| !a.ready()) || (account.is_none() && protected) {
            return Err(rejected(
                if account.is_some() {
                    "collected_account_not_ready"
                } else {
                    "collected_account_missing"
                },
                "account_ready",
                FundingError::Unavailable,
            ));
        }
        if let Some(account) = &account {
            // 同一持久策略决定现金或纯记账证明；缺口只提交冻结，不满足付款或解除依赖。
            if !settlement_funding::check_funding_evidence(tx, account, evidence).await.map_err(
                |error| rejected("collected_funding_evidence_failed", "funding_evidence", error),
            )? {
                crate::validate_mail_evidence_time(tx, evidence)
                    .await
                    .map_err(evidence_error)
                    .map_err(|error| {
                        rejected(
                            "collected_deferred_evidence_time_failed",
                            "funding_deferred",
                            error,
                        )
                    })?;
                tracing::warn!(
                    target: settlement_funding::diagnostics::TARGET,
                    safe_kind = "collected_cash_gate_deferred",
                    stage = "funding_evidence",
                    "COD 资金门延后，等待原事务提交结果"
                );
                return Ok(true);
            }
        }
        // 前置现金门通过后才写原满足事实；发行后的覆盖校验失败仍整事务回滚。
        persistence::satisfy(tx, command, evidence, Some(proof))
            .await
            .map_err(|error| rejected("collected_satisfy_write_failed", "satisfy_write", error))?;
        if let Some(account) = &account {
            let receipt = FundingCollectedReceipt {
                payment_obligation_id: command.claim.obligation_id.as_uuid(),
                delivery_id: command.claim.delivery_id.ok_or_else(|| {
                    rejected(
                        "collected_receipt_delivery_missing",
                        "receipt_input",
                        FundingError::Unproven,
                    )
                })?,
                buyer_credits: command.claim.amount,
                evidence,
            };
            settlement_funding::fund_collected_payment(tx, account, &mutation, &receipt)
                .await
                .map_err(|error| {
                    rejected("collected_fund_payment_failed", "fund_payment", error)
                })?;
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
    persistence::complete(tx, command, evidence, proof.is_some(), external_refund)
        .await
        .map_err(|error| rejected("collected_completion_failed", "completion", error))?;
    Ok(false)
}

async fn validate_collected(
    tx: &mut Transaction<'_, Postgres>,
    command: &SettlementInternalCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
    proof: &SettlementFulfillmentProof,
) -> Result<(), FundingError> {
    let c = &command.claim;
    let rejected =
        |kind, error| settlement_funding::diagnostics::reject(kind, "collected_proof", error);
    let sent = evidence
        .sent_collection
        .as_ref()
        .ok_or_else(|| rejected("collected_proof_sent_missing", FundingError::Unproven))?;
    let delivery = c
        .delivery_snapshot
        .as_ref()
        .ok_or_else(|| rejected("collected_proof_delivery_missing", FundingError::Unproven))?;
    // 保留原短路顺序，包括只有前面条件通过才读取当前时钟与构造主题。
    let rejection = if proof.kind != "mooglemail_sent_collected" {
        Some("collected_proof_kind_invalid")
    } else if proof.digest == [0; 32] {
        Some("collected_proof_digest_missing")
    } else if proof.reference != sent.external_mail_id {
        Some("collected_proof_reference_mismatch")
    } else if sent.recipient != c.recipient {
        Some("collected_proof_recipient_mismatch")
    } else if sent.binding.world != c.world {
        Some("collected_proof_world_mismatch")
    } else if !matches!(
        c.kind,
        auction_domain::SettlementObligationKind::WinnerEquipmentCod
            | auction_domain::SettlementObligationKind::WinnerPaymentCarrierCod
    ) {
        Some("collected_proof_obligation_kind_invalid")
    } else if proof.observed_at < c.claimed_at {
        Some("collected_proof_observation_before_claim")
    } else if proof.observed_at > chrono::Utc::now() {
        Some("collected_proof_observation_in_future")
    } else if auction_application::settlement::canonical_settlement_fulfillment_subject(
        c.kind,
        c.obligation_id,
    )
    .as_deref()
        != Some(sent.subject.as_str())
    {
        Some("collected_proof_subject_mismatch")
    } else {
        None
    };
    if let Some(kind) = rejection {
        return Err(rejected(kind, FundingError::Unproven));
    }
    let prepare_digest = c.prepare_request_hash.map(|hash| hash.as_bytes());
    let activate_digest = c.activate_request_hash.map(|hash| hash.as_bytes());
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
        .bind(c.prepare_command_id.map(|id| id.as_uuid())).bind(prepare_digest.as_ref().map(|value| value.as_slice()))
        .bind(c.activate_command_id.map(|id| id.as_uuid())).bind(activate_digest.as_ref().map(|value| value.as_slice()))
        .bind(format!("mooglemail:{}:send:{}",c.world.as_str(),sent.external_mail_id))
        .fetch_one(&mut **tx).await.map_err(sql_error)
        .map_err(|error| rejected("collected_proof_query_failed", error))?;
    if !proven {
        return Err(rejected("collected_proof_binding_unproven", FundingError::Unproven));
    }
    Ok(())
}

fn diagnose_collected(
    collected: bool,
    kind: &'static str,
    stage: &'static str,
    error: FundingError,
) -> FundingError {
    if collected { settlement_funding::diagnostics::reject(kind, stage, error) } else { error }
}
