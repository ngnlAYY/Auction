//! 缺账户→record_only的唯一初始化；旧strict不转换，初始旧责任只登记一次quarantine。

use super::*;
use auction_application::InternalEvidencePurpose;
use auction_domain::SettlementWorld;
use serde_json::json;

pub(super) async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    command: &FundingRecordOnlyCommand,
) -> Result<Option<FundingReconciliationResult>, FundingError> {
    let row=sqlx::query("SELECT uid,action,request_digest,result FROM public.settlement_funding_commands WHERE command_id=$1")
        .bind(command.command_id).fetch_optional(&mut **tx).await.map_err(storage)?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<String, _>("uid").map_err(storage)? != command.expected_uid
        || row.try_get::<String, _>("action").map_err(storage)? != "record_only_init"
        || row.try_get::<Vec<u8>, _>("request_digest").map_err(storage)?
            != command.request_digest()?
    {
        return Err(FundingError::Conflict);
    }
    serde_json::from_value(row.try_get("result").map_err(storage)?).map(Some).map_err(invalid)
}

pub(super) async fn run(
    pool: &crate::database::TrackedPool<Postgres>,
    command: &FundingRecordOnlyCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<FundingCommitOutcome, FundingError> {
    pool.observe_transaction(auction_observability::DatabaseTransactionOwner::Funding, async {
        command.validate()?;
        for attempt in 0..3 {
            let mut tx =
                pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(storage)?;
            let operation = async {
                if let Some(result) = replay(&mut tx, command).await? {
                    return Ok(result);
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
                    Ok(()) => return Ok(FundingCommitOutcome::Committed(result)),
                    Err(error) => match storage(error) {
                        FundingError::SerializationConflict if attempt < 2 => continue,
                        FundingError::SerializationConflict => {
                            auction_observability::database_transaction_exhausted();
                            return Ok(FundingCommitOutcome::RolledBack(
                                FundingError::SerializationConflict,
                            ));
                        }
                        error => return Err(error),
                    },
                },
                Err(error) => {
                    tx.rollback().await.map_err(storage)?;
                    if matches!(error, FundingError::SerializationConflict) && attempt < 2 {
                        continue;
                    }
                    crate::database::observe_retry_exhaustion(
                        attempt + 1,
                        3,
                        matches!(error, FundingError::SerializationConflict),
                    );
                    return Ok(FundingCommitOutcome::RolledBack(error));
                }
            }
        }
        Ok(FundingCommitOutcome::RolledBack(FundingError::SerializationConflict))
    })
    .await
}

async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    command: &FundingRecordOnlyCommand,
    e: &SettlementInternalEvidenceSnapshot,
) -> Result<FundingReconciliationResult, FundingError> {
    if !e.valid_at(chrono::Utc::now())
        || e.operation_id != command.command_id
        || e.identity.authenticated_uid != command.expected_uid
        || e.identity.world != SettlementWorld::Main
        || e.main_cash.is_some()
        || e.equipment.is_some()
        || e.sent_collection.is_some()
        || !matches!(&e.purpose,InternalEvidencePurpose::FundingRecordOnlyInitialization{command_id,expected_account_version}
            if *command_id==command.command_id && *expected_account_version==0)
    {
        return Err(FundingError::Unproven);
    }
    crate::lock_mail_runtime_generation(tx, &e.identity.runtime).await.map_err(evidence_error)?;
    lock_funding_worlds(tx).await?;
    if lock_funding_account(tx, &command.expected_uid).await?.is_some() {
        return Err(FundingError::Conflict);
    }
    // 持全局scope后拒绝其他账户，避免创建后与既有strict/其他UID混用同一mail_cookie。
    let any_account: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.settlement_funding_accounts)")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage)?;
    if any_account {
        return Err(FundingError::Conflict);
    }
    lock_funding_liabilities(tx, e).await?;
    if let Some(result) = replay(tx, command).await? {
        return Ok(result);
    }
    let preview = super::super::preview::snapshot(tx, &command.expected_uid).await?;
    if preview.account_status != "unreconciled"
        || preview.account_version != 0
        || preview.digest != command.preview_digest
    {
        return Err(FundingError::Conflict);
    }
    if !preview.blockers.is_empty() || preview.allocated_local != 0 {
        return Err(FundingError::Unproven);
    }
    crate::validate_mail_evidence_time(tx, e).await.map_err(evidence_error)?;
    sqlx::query("INSERT INTO public.settlement_funding_accounts(uid,accounting_mode,status,state_version) VALUES($1,'record_only','ready',1)")
        .bind(&command.expected_uid).execute(&mut **tx).await.map_err(storage)?;
    // 初始source仅记录当前已知旧责任，不声称隔离了真实现金，也不授予P/C预算。
    let source = ledger::source(
        tx,
        &command.expected_uid,
        "record_opening",
        command.command_id,
        preview.isolated_liabilities,
        &e.digest,
    )
    .await?;
    let quarantine =
        ledger::allocation(tx, &command.expected_uid, "quarantine", None, None).await?;
    let mutation = FundingMutation {
        command_id: command.command_id,
        request_digest: command.request_digest()?,
    };
    ledger::issue(
        tx,
        &command.expected_uid,
        &mutation,
        source,
        quarantine,
        preview.isolated_liabilities,
    )
    .await?;
    let result = FundingReconciliationResult {
        command_id: command.command_id,
        uid: command.expected_uid.clone(),
        account_version: 1,
        source_id: Some(source),
        account_status: "ready".to_owned(),
        allocated_credits: preview.isolated_liabilities,
    };
    let binding = &e.identity;
    let evidence = json!({"operation_id":e.operation_id,"purpose":{"FundingRecordOnlyInitialization":{
        "command_id":command.command_id,"expected_account_version":0}},
        "identity":{"credential_id":binding.credential_id,"credential_version":binding.credential_version,
            "runtime":{"generation":binding.runtime.generation},"authenticated_uid":binding.authenticated_uid,
            "player_name":binding.player_name,"world":binding.world,"observed_at":binding.observed_at,
            "protocol_version":binding.protocol_version,"digest":binding.digest},
        "equipment":null,"main_cash":null,"sent_collection":null,
        "observed_from":e.observed_from,"expires_at":e.expires_at,"digest":e.digest});
    let metadata = auction_application::cli_audit_metadata(
        command.operator_identity.as_ref(),
        json!({"command_id":command.command_id,"accounting_mode":"record_only","policy_version":1,
        "source_id":source,"recorded_liabilities":preview.isolated_liabilities,"preview_digest":preview.digest,"evidence_digest":e.digest,
        "executor":"auction-cli"}),
    );
    sqlx::query("INSERT INTO public.settlement_funding_commands(command_id,uid,action,target_id,request_digest,result,\"authorization\") VALUES($1,$2,'record_only_init',$1,$3,$4,$5)")
        .bind(command.command_id).bind(&command.expected_uid).bind(mutation.request_digest.as_slice())
        .bind(serde_json::to_value(&result).map_err(invalid)?)
        .bind(json!({"accounting_mode":"record_only","policy_version":1,"full_snapshot":evidence,"facts":preview.facts}))
        .execute(&mut **tx).await.map_err(storage)?;
    // 显式 CLI 身份决定来源；未携带身份的 Admin 调用保持原来源，不改写历史事实。
    sqlx::query("INSERT INTO public.audit_records(id,occurred_at,actor_type,actor_id,action,entity_type,entity_id,result,source,metadata) VALUES($1,clock_timestamp(),'admin',$2,'auction.funding.record_only_initialized','funding_account',$3,'success',$5,$4)")
        .bind(command.command_id).bind(&command.actor).bind(&command.expected_uid).bind(metadata)
        .bind(if command.operator_identity.is_some() { "cli" } else { "admin" })
        .execute(&mut **tx).await.map_err(storage)?;
    crate::validate_mail_evidence_time(tx, e).await.map_err(evidence_error)?;
    Ok(result)
}
