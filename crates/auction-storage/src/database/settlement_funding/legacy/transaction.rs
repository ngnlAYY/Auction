//! 同一可信会话与 Serializable 事务内认领；全程只转移已有隔离，不发行收入。

use auction_application::{FundingCollectedReceipt, FundingMutation, InternalEvidencePurpose};
use auction_domain::SettlementWorld;
use sqlx::{PgPool, Postgres, Row, Transaction};

use super::{super::*, *};

pub(super) async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    command: &LegacyBotSellerCommand,
) -> Result<Option<LegacyBotSellerResult>, FundingError> {
    let row = sqlx::query("SELECT uid,obligation_id,request_digest,result FROM public.settlement_legacy_bot_seller_claims WHERE command_id=$1")
        .bind(command.command_id).fetch_optional(&mut **tx).await.map_err(storage)?;
    let Some(row) = row else {
        let deferred = sqlx::query("SELECT uid,action,target_id,request_digest FROM public.settlement_funding_commands WHERE command_id=$1")
            .bind(command.command_id).fetch_optional(&mut **tx).await.map_err(storage)?;
        if let Some(row) = deferred {
            if row.try_get::<String, _>("uid").map_err(storage)? != command.expected_uid
                || row.try_get::<String, _>("action").map_err(storage)?
                    != "legacy_bot_seller_deferred"
                || row.try_get::<uuid::Uuid, _>("target_id").map_err(storage)?
                    != command.obligation_id
                || row.try_get::<Vec<u8>, _>("request_digest").map_err(storage)?
                    != command.request_digest()?
            {
                return Err(FundingError::Conflict);
            }
            // 原命令已确定只冻结/保留占用；修复资金后需新预览与显式新授权，不能重用旧ID执行认领。
            return Err(FundingError::Unavailable);
        }
        return Ok(None);
    };
    if row.try_get::<String, _>("uid").map_err(storage)? != command.expected_uid
        || row.try_get::<uuid::Uuid, _>("obligation_id").map_err(storage)? != command.obligation_id
        || row.try_get::<Vec<u8>, _>("request_digest").map_err(storage)?
            != command.request_digest()?
    {
        return Err(FundingError::Conflict);
    }
    serde_json::from_value(row.try_get("result").map_err(storage)?).map(Some).map_err(invalid)
}

pub(super) async fn run(
    pool: &PgPool,
    command: &LegacyBotSellerCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<LegacyBotSellerCommitOutcome, FundingError> {
    command.validate()?;
    for attempt in 0..3 {
        let mut tx =
            pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(storage)?;
        let operation = async {
            if let Some(result) = replay(&mut tx, command).await? {
                return Ok(Some(result));
            }
            let budget = (evidence.expires_at - chrono::Utc::now()).num_milliseconds();
            if budget <= 0 {
                return Err(FundingError::Unproven);
            }
            sqlx::query("SELECT set_config('transaction_timeout',$1,true)")
                .bind(format!("{budget}ms"))
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            apply(&mut tx, command, evidence).await
        }
        .await;
        match operation {
            Ok(result) => match tx.commit().await {
                Ok(()) => {
                    return Ok(match result {
                        Some(result) => LegacyBotSellerCommitOutcome::Committed(result),
                        None => LegacyBotSellerCommitOutcome::FundingDeferred,
                    });
                }
                Err(error) => match storage(error) {
                    FundingError::SerializationConflict if attempt < 2 => continue,
                    FundingError::SerializationConflict => {
                        return Ok(LegacyBotSellerCommitOutcome::RolledBack(
                            FundingError::SerializationConflict,
                        ));
                    }
                    error => return Err(error),
                },
            },
            Err(error) => {
                // 未知COMMIT/ROLLBACK只返回Err，让opaque guard撤销owner，不能解除会话继续写。
                tx.rollback().await.map_err(storage)?;
                if matches!(error, FundingError::SerializationConflict) && attempt < 2 {
                    continue;
                }
                return Ok(LegacyBotSellerCommitOutcome::RolledBack(error));
            }
        }
    }
    Ok(LegacyBotSellerCommitOutcome::RolledBack(FundingError::SerializationConflict))
}

async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    command: &LegacyBotSellerCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<Option<LegacyBotSellerResult>, FundingError> {
    let expected = u64::try_from(command.expected_state_version).map_err(invalid)?;
    if !evidence.valid_at(chrono::Utc::now())
        || evidence.operation_id != command.command_id
        || evidence.identity.authenticated_uid != command.expected_uid
        || evidence.identity.world != SettlementWorld::Main
        || evidence.equipment.is_some()
        || !matches!(&evidence.purpose, InternalEvidencePurpose::LegacyBotSeller {command_id,obligation_id,expected_state_version}
            if *command_id==command.command_id && *obligation_id==command.obligation_id && *expected_state_version==expected)
    {
        return Err(FundingError::Unproven);
    }
    let sent = evidence.sent_collection.as_ref().ok_or(FundingError::Unproven)?;
    crate::lock_mail_runtime_generation(tx, &evidence.identity.runtime)
        .await
        .map_err(evidence_error)?;
    // 与开户和普通履约使用同一个锁前缀；低频修复锁完整责任集合，防跨lot重复认领。
    lock_rows(tx, "SELECT id FROM public.worlds ORDER BY id FOR UPDATE").await?;
    let account =
        lock_funding_account(tx, &command.expected_uid).await?.ok_or(FundingError::Unavailable)?;
    if !account.ready() {
        return Err(FundingError::Unavailable);
    }
    lock_rows(tx, "SELECT id FROM public.auctions ORDER BY id FOR UPDATE").await?;
    lock_rows(tx, "SELECT id FROM public.settlement_recoveries ORDER BY id FOR UPDATE").await?;
    lock_rows(tx, "SELECT id FROM public.lots ORDER BY id FOR UPDATE").await?;
    lock_rows(tx, "SELECT id FROM public.equipment ORDER BY id FOR UPDATE").await?;
    crate::lock_mail_evidence_credentials(tx, evidence).await.map_err(evidence_error)?;
    lock_rows(tx, "SELECT id FROM public.users ORDER BY id FOR UPDATE").await?;
    lock_rows(tx, "SELECT id FROM public.settlement_obligations ORDER BY id FOR UPDATE").await?;
    lock_rows(tx, "SELECT id FROM public.settlement_recovery_actions ORDER BY id FOR UPDATE")
        .await?;
    lock_rows(tx, "SELECT id FROM public.deliveries ORDER BY id FOR UPDATE").await?;
    lock_rows(tx,"SELECT obligation_id FROM public.settlement_obligation_fulfillments ORDER BY obligation_id FOR UPDATE").await?;
    if let Some(result) = replay(tx, command).await? {
        return Ok(Some(result));
    }
    let target = super::preview::load(tx, command.obligation_id, &command.expected_uid).await?;
    let p = &target.preview;
    if p.state_version != command.expected_state_version || p.digest != command.expected_digest {
        return Err(FundingError::Conflict);
    }
    if evidence.identity.player_name != target.seller_name
        || sent.external_mail_id != p.payment_external_mail_id
        || sent.recipient != p.payment_recipient
        || sent.subject != p.payment_subject
        || sent.binding.world != SettlementWorld::Main
        || target.facts["quarantine_state"] != "available"
        || target.facts["quarantine_amount"]
            .as_i64()
            .is_none_or(|amount| amount < p.quarantine_required)
    {
        return Err(FundingError::Unproven);
    }
    receipt::validate_legacy_receipt(
        tx,
        &account,
        &FundingCollectedReceipt {
            payment_obligation_id: p.payment_obligation_id,
            delivery_id: p.payment_delivery_id,
            buyer_credits: p.buyer_amount,
            evidence,
        },
        p.payment_obligation_id,
        p.buyer_amount,
    )
    .await?;
    crate::validate_mail_evidence_time(tx, evidence).await.map_err(evidence_error)?;
    if !check_funding_evidence(tx, &account, evidence).await? {
        sqlx::query("INSERT INTO public.settlement_funding_commands(command_id,uid,action,target_id,request_digest,result,\"authorization\") VALUES($1,$2,'legacy_bot_seller_deferred',$3,$4,$5,$6)")
            .bind(command.command_id).bind(account.uid()).bind(command.obligation_id).bind(command.request_digest()?.as_slice())
            .bind(serde_json::json!({"outcome":"funding_deferred"}))
            .bind(serde_json::json!({"evidence_digest":evidence.digest,"observed_credits":evidence.main_cash.as_ref().map(|cash|cash.credits),
                "snapshot_digest":command.expected_digest,"full_snapshot":super::record::evidence_summary(evidence)}))
            .execute(&mut **tx).await.map_err(storage)?;
        crate::validate_mail_evidence_time(tx, evidence).await.map_err(evidence_error)?;
        return Ok(None);
    }
    // 已隔离的全局上界必须覆盖所有旧责任，不能从不足的隔离中优先认领一笔。
    let global = super::super::preview::snapshot(tx, &command.expected_uid).await?;
    if (!account.records_only() && !global.blockers.is_empty()) || global.isolated_liabilities != 0
    {
        return Err(FundingError::Unproven);
    }
    let mutation = FundingMutation {
        command_id: command.command_id,
        request_digest: command.request_digest()?,
    };
    super::record::retain(tx, &account, &mutation, command, &target, evidence).await.map(Some)
}

async fn lock_rows(
    tx: &mut Transaction<'_, Postgres>,
    query: &'static str,
) -> Result<(), FundingError> {
    sqlx::query(query).execute(&mut **tx).await.map_err(storage)?;
    Ok(())
}
