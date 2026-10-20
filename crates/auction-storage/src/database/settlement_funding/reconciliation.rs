//! 显式对账提交；完整责任快照CAS、真实现金与用途授权同Serializable事务。

use super::{ledger::*, *};
use auction_application::{
    FundingCashGate, FundingCommitOutcome, FundingFuture, FundingReconciliationCommand,
    FundingReconciliationResult, FundingReconciliationWriter, InternalEvidencePurpose,
    SettlementInternalEvidenceSnapshot,
};
use auction_domain::{SettlementAmount, SettlementWorld, funding_reconciliation_capacity};
use sha2::{Digest, Sha256};

fn command_digest(command: &FundingReconciliationCommand) -> Result<[u8; 32], FundingError> {
    Ok(Sha256::digest(serde_json::to_vec(command).map_err(invalid)?).into())
}

pub(super) async fn replay_command(
    tx: &mut Transaction<'_, Postgres>,
    command: &FundingReconciliationCommand,
) -> Result<Option<FundingReconciliationResult>, FundingError> {
    let row=sqlx::query("SELECT uid,action,request_digest,result FROM public.settlement_funding_commands WHERE command_id=$1")
        .bind(command.command_id).fetch_optional(&mut **tx).await.map_err(storage)?;
    let Some(row) = row else { return Ok(None) };
    if row.try_get::<String, _>("uid").map_err(storage)? != command.expected_uid
        || row.try_get::<String, _>("action").map_err(storage)? != "reconcile"
        || row.try_get::<Vec<u8>, _>("request_digest").map_err(storage)? != command_digest(command)?
    {
        return Err(FundingError::Conflict);
    }
    serde_json::from_value(row.try_get("result").map_err(storage)?).map(Some).map_err(invalid)
}

impl FundingReconciliationWriter for crate::DatabaseStore {
    fn reconcile_funding<'a>(
        &'a self,
        command: &'a FundingReconciliationCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
        _gate: FundingCashGate,
    ) -> FundingFuture<'a, FundingCommitOutcome> {
        Box::pin(async move {
            command.validate()?;
            for attempt in 0..3 {
                let mut tx = self.postgres_pool().begin().await.map_err(storage)?;
                sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                    .execute(&mut *tx)
                    .await
                    .map_err(storage)?;
                match apply(&mut tx, command, evidence).await {
                    Ok(result) => match tx.commit().await {
                        Ok(()) => return Ok(FundingCommitOutcome::Committed(result)),
                        Err(error) => match storage(error) {
                            FundingError::SerializationConflict if attempt < 2 => continue,
                            FundingError::SerializationConflict => {
                                return Ok(FundingCommitOutcome::RolledBack(
                                    FundingError::SerializationConflict,
                                ));
                            }
                            error => return Err(error),
                        },
                    },
                    Err(error) => {
                        // 只有显式ROLLBACK已确认才允许释放而不撤销owner。
                        tx.rollback().await.map_err(storage)?;
                        if matches!(error, FundingError::SerializationConflict) && attempt < 2 {
                            continue;
                        }
                        return Ok(FundingCommitOutcome::RolledBack(error));
                    }
                }
            }
            Ok(FundingCommitOutcome::RolledBack(FundingError::SerializationConflict))
        })
    }
}

async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    command: &FundingReconciliationCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<FundingReconciliationResult, FundingError> {
    if let Some(result) = replay_command(tx, command).await? {
        return Ok(result);
    }
    let expected = u64::try_from(command.expected_version).map_err(invalid)?;
    if evidence.operation_id != command.command_id
        || evidence.identity.authenticated_uid != command.expected_uid
        || evidence.identity.world != SettlementWorld::Main
        || !matches!(&evidence.purpose,InternalEvidencePurpose::FundingReconciliation {command_id,expected_account_version}
            if *command_id==command.command_id && *expected_account_version==expected)
    {
        return Err(FundingError::Unproven);
    }
    let cash = evidence.main_cash.as_ref().ok_or(FundingError::Unproven)?;
    if cash.binding.authenticated_uid != command.expected_uid || cash.credits < 0 {
        return Err(FundingError::Unproven);
    }
    crate::lock_mail_runtime_generation(tx, &evidence.identity.runtime)
        .await
        .map_err(evidence_error)?;
    // 完整对账是低频全局操作，按统一顺序锁完整财务责任集合而不是抽样。
    lock_rows(tx, "SELECT id FROM public.worlds ORDER BY id FOR UPDATE").await?;
    let account = lock_funding_account(tx, &command.expected_uid).await?;
    if account.as_ref().is_some_and(LockedFundingAccount::records_only) {
        return Err(FundingError::Conflict);
    }
    // scope锁覆盖反向开户：已有record_only时旧strict入口也不能新建另一UID形成混用。
    let record_only_exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.settlement_funding_accounts WHERE accounting_mode='record_only')")
        .fetch_one(&mut **tx).await.map_err(storage)?;
    if record_only_exists {
        return Err(FundingError::Conflict);
    }
    if account.as_ref().map_or(0, LockedFundingAccount::version) != command.expected_version {
        return Err(FundingError::Conflict);
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
    let preview = super::preview::snapshot(tx, &command.expected_uid).await?;
    if preview.digest != command.preview_digest
        || preview.account_version != command.expected_version
    {
        return Err(FundingError::Conflict);
    }
    crate::validate_mail_evidence_time(tx, evidence).await.map_err(evidence_error)?;
    if cash.credits < preview.allocated_local {
        let version:i64=sqlx::query_scalar("UPDATE public.settlement_funding_accounts SET status='frozen',state_version=state_version+1,updated_at=clock_timestamp() WHERE uid=$1 RETURNING state_version")
            .bind(&command.expected_uid).fetch_one(&mut **tx).await.map_err(storage)?;
        let result = FundingReconciliationResult {
            command_id: command.command_id,
            uid: command.expected_uid.clone(),
            account_version: version,
            source_id: None,
            account_status: "frozen".to_owned(),
            allocated_credits: 0,
        };
        let authorization = serde_json::json!({"command":command,"reason":"cash_shortfall","observed_credits":cash.credits,
            "allocated_local":preview.allocated_local,"evidence_digest":evidence.digest});
        record_result(tx, command, &result, authorization).await?;
        return Ok(result);
    }
    if !preview.blockers.is_empty() {
        return Err(FundingError::Unproven);
    }
    if command.compensation_credits > preview.unfunded_compensation {
        return Err(FundingError::Invalid);
    }
    funding_reconciliation_capacity(
        SettlementAmount::new(cash.credits).map_err(invalid)?,
        SettlementAmount::new(preview.allocated_local).map_err(invalid)?,
        SettlementAmount::new(preview.isolated_liabilities).map_err(invalid)?,
        SettlementAmount::new(command.proprietary_credits).map_err(invalid)?,
        SettlementAmount::new(command.compensation_credits).map_err(invalid)?,
    )
    .map_err(|_| FundingError::Unavailable)?;
    if account.is_none() {
        sqlx::query("INSERT INTO public.settlement_funding_accounts(uid) VALUES($1)")
            .bind(&command.expected_uid)
            .execute(&mut **tx)
            .await
            .map_err(storage)?;
    }
    let amount = preview
        .isolated_liabilities
        .checked_add(command.proprietary_credits)
        .and_then(|value| value.checked_add(command.compensation_credits))
        .ok_or(FundingError::Invalid)?;
    // 首次零额度也建立明确边界；已有账户的纯复核不伪造一次opening调整。
    let source_id = if amount > 0 || account.is_none() {
        Some(
            source(
                tx,
                &command.expected_uid,
                "reconciliation",
                command.command_id,
                amount,
                &evidence.digest,
            )
            .await?,
        )
    } else {
        None
    };
    let mutation = FundingMutation {
        command_id: command.command_id,
        request_digest: command_digest(command)?,
    };
    if let Some(source_id) = source_id {
        for (purpose, amount) in [
            ("quarantine", preview.isolated_liabilities),
            ("proprietary", command.proprietary_credits),
            ("compensation", command.compensation_credits),
        ] {
            let target = allocation(tx, &command.expected_uid, purpose, None, None).await?;
            issue(tx, &command.expected_uid, &mutation, source_id, target, amount).await?;
        }
    }
    crate::validate_mail_evidence_time(tx, evidence).await.map_err(evidence_error)?;
    let version:i64=sqlx::query_scalar("UPDATE public.settlement_funding_accounts SET status='ready',state_version=state_version+1,updated_at=clock_timestamp() WHERE uid=$1 AND state_version=$2 RETURNING state_version")
        .bind(&command.expected_uid).bind(command.expected_version).fetch_one(&mut **tx).await.map_err(storage)?;
    let result = FundingReconciliationResult {
        command_id: command.command_id,
        uid: command.expected_uid.clone(),
        account_version: version,
        source_id,
        account_status: "ready".to_owned(),
        allocated_credits: amount,
    };
    let authorization = serde_json::json!({"command":command,"evidence":{
        "credential_id":evidence.identity.credential_id,"credential_version":evidence.identity.credential_version,
        "runtime_generation":evidence.identity.runtime.generation,"uid":evidence.identity.authenticated_uid,
        "player_name":evidence.identity.player_name,"protocol":evidence.identity.protocol_version,
        "observed_from":evidence.observed_from,"expires_at":evidence.expires_at,"digest":evidence.digest,
        "cash_credits":cash.credits,"cash_observed_at":cash.observed_at,"cash_digest":cash.digest,
        "isolated_liabilities":preview.isolated_liabilities,"allocated_local":preview.allocated_local}});
    record_result(tx, command, &result, authorization).await?;
    Ok(result)
}

async fn record_result(
    tx: &mut Transaction<'_, Postgres>,
    command: &FundingReconciliationCommand,
    result: &FundingReconciliationResult,
    authorization: serde_json::Value,
) -> Result<(), FundingError> {
    // 历史 schema 保留 authorization 列名，引用时必须明确加引号。
    sqlx::query("INSERT INTO public.settlement_funding_commands(command_id,uid,action,target_id,request_digest,result,\"authorization\") VALUES($1,$2,'reconcile',$1,$3,$4,$5)")
        .bind(command.command_id).bind(&command.expected_uid).bind(command_digest(command)?.as_slice())
        .bind(serde_json::to_value(result).map_err(invalid)?).bind(authorization).execute(&mut **tx).await.map_err(storage)?;
    Ok(())
}

async fn lock_rows(
    tx: &mut Transaction<'_, Postgres>,
    query: &'static str,
) -> Result<(), FundingError> {
    sqlx::query(query).fetch_all(&mut **tx).await.map_err(storage)?;
    Ok(())
}
