//! 有界 Serializable 核验、原记录终结和追加事实必须同事务；不生成付款/收款事实。

use super::*;
use auction_application::{InternalEvidencePurpose, LegacySelfReconciliationAction as Action};
use serde_json::{Value, json};

pub(super) async fn run(
    pool: &sqlx::PgPool,
    command: &LegacySelfReconciliationCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<LegacySelfReconciliationOutcome, FundingError> {
    command.validate()?;
    for attempt in 0..3 {
        let mut tx = pool.begin().await.map_err(sql_error)?;
        let operation = async {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                .execute(&mut *tx)
                .await
                .map_err(sql_error)?;
            if let Some(result) = read::replay(&mut tx, command).await? {
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
                .map_err(sql_error)?;
            apply(&mut tx, command, evidence).await
        }
        .await;
        match operation {
            Ok(result) => match tx.commit().await {
                Ok(()) => return Ok(LegacySelfReconciliationOutcome::Committed(result)),
                Err(error) => match sql_error(error) {
                    FundingError::SerializationConflict if attempt < 2 => continue,
                    FundingError::SerializationConflict => {
                        return Ok(LegacySelfReconciliationOutcome::RolledBack(
                            FundingError::SerializationConflict,
                        ));
                    }
                    error => return Err(error),
                },
            },
            Err(error) => {
                tx.rollback().await.map_err(sql_error)?;
                if matches!(error, FundingError::SerializationConflict) && attempt < 2 {
                    continue;
                }
                return Ok(LegacySelfReconciliationOutcome::RolledBack(error));
            }
        }
    }
    Ok(LegacySelfReconciliationOutcome::RolledBack(FundingError::SerializationConflict))
}

async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    command: &LegacySelfReconciliationCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<LegacySelfReconciliationResult, FundingError> {
    let target = &command.target;
    let presence = evidence.equipment.as_ref().ok_or(FundingError::Unproven)?;
    if !evidence.valid_at(chrono::Utc::now())
        || evidence.operation_id != command.command_id
        || evidence.main_cash.is_some()
        || evidence.sent_collection.is_some()
        || evidence.identity.authenticated_uid != command.expected_uid
        || evidence.identity.player_name != target.recipient
        || evidence.identity.world != target.world
        || presence.equipment_id != target.equipment_id
        || presence.external_equipment_id != target.external_equipment_id
        || presence.binding.world != target.world
        || !matches!(&evidence.purpose,InternalEvidencePurpose::LegacySelfReconciliation {
            command_id,obligation_id,expected_fulfillment_version
        } if *command_id==command.command_id && *obligation_id==target.obligation_id
            && *expected_fulfillment_version==target.expected_fulfillment_version)
    {
        return Err(FundingError::Unproven);
    }
    crate::lock_mail_runtime_generation(tx, &evidence.identity.runtime)
        .await
        .map_err(evidence_error)?;
    lock_rows(tx, "SELECT id FROM public.worlds ORDER BY id FOR UPDATE").await?;
    if super::super::settlement_funding::lock_funded_delivery_account(tx, target.delivery_id)
        .await?
        .is_some()
    {
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
    if let Some(result) = read::replay(tx, command).await? {
        return Ok(result);
    }
    let (mut original, current) = read::snapshot(tx, target.obligation_id).await?;
    if current != *target {
        return Err(FundingError::Conflict);
    }
    let lot_id = original["obligation"]["lot_id"]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(FundingError::Invalid)?;
    super::super::fulfillment_scheduler::exclude_legacy_self_asset_conflicts(tx, target, lot_id)
        .await?;
    crate::validate_mail_evidence_time(tx, evidence).await.map_err(evidence_error)?;
    let marker = json!({"command_id":command.command_id,"action":target.action.as_str(),"evidence_digest":evidence.digest});
    let changed=sqlx::query(
        "UPDATE public.deliveries SET status='failed',fencing_version=fencing_version+1,
          result_metadata=result_metadata||jsonb_build_object('legacy_self_reconciliation',$3::jsonb),
          updated_at=clock_timestamp()
         WHERE id=$1 AND status='manual_reconcile' AND fencing_version=$2 AND lease_token IS NULL"
    ).bind(target.delivery_id).bind(target.expected_delivery_fencing).bind(marker)
        .execute(&mut **tx).await.map_err(sql_error)?;
    if changed.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    let result = resolve_fulfillment(tx, command, evidence).await?;
    original["target"] = serde_json::to_value(target).map_err(|_| FundingError::Invalid)?;
    let evidence_json = evidence_summary(evidence);
    sqlx::query(
        "INSERT INTO public.settlement_legacy_self_reconciliations
         (command_id,delivery_id,obligation_id,action,request_digest,uid,expected_delivery_fencing,
          expected_fulfillment_version,expected_obligation_version,snapshot_digest,original_snapshot,
          evidence_digest,evidence,result,actor_id,reason)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)"
    ).bind(command.command_id).bind(target.delivery_id).bind(target.obligation_id).bind(target.action.as_str())
        .bind(command.request_digest()?.to_vec()).bind(&command.expected_uid).bind(target.expected_delivery_fencing)
        .bind(target.expected_fulfillment_version).bind(target.expected_obligation_version).bind(target.snapshot_digest.to_vec())
        .bind(original).bind(evidence.digest.to_vec()).bind(evidence_json).bind(serde_json::to_value(&result).map_err(|_| FundingError::Invalid)?)
        .bind(&command.actor).bind(&command.reason).execute(&mut **tx).await.map_err(sql_error)?;
    let metadata = json!({"command_id":command.command_id,"delivery_id":target.delivery_id,
        "obligation_id":target.obligation_id,"action":target.action.as_str(),"uid":command.expected_uid,
        "snapshot_digest":target.snapshot_digest,"evidence_digest":evidence.digest,"reason":command.reason,
        "executor":"auctionctl"});
    // 审计来源使用既有角色闭集；具体执行器单独保留在 metadata，不能作为新 source 写入。
    sqlx::query("INSERT INTO public.audit_records(id,occurred_at,actor_type,actor_id,action,entity_type,entity_id,result,source,metadata) VALUES($1,clock_timestamp(),'admin',$2,'auction.fulfillment.legacy_self_reconciled','settlement_obligation',$3,'success','admin',$4)")
        .bind(command.command_id).bind(&command.actor).bind(target.obligation_id.to_string()).bind(metadata)
        .execute(&mut **tx).await.map_err(sql_error)?;
    super::super::delivery_audit::append(
        tx,
        super::super::delivery_audit::DeliveryAudit {
            occurred_at: chrono::Utc::now(),
            actor_type: "admin",
            actor_id: &command.actor,
            action: super::super::delivery_audit::ADMIN_RECONCILE,
            delivery_id: target.delivery_id,
            request_id: None,
            source: "admin",
            metadata: json!({"command_id":command.command_id,
            "resolution":"legacy_self_reconciled","action":target.action.as_str(),
            "fencing_version":target.expected_delivery_fencing+1,"executor":"auctionctl"}),
        },
    )
    .await
    .map_err(sql_error)?;
    crate::validate_mail_evidence_time(tx, evidence).await.map_err(evidence_error)?;
    Ok(result)
}

async fn resolve_fulfillment(
    tx: &mut Transaction<'_, Postgres>,
    command: &LegacySelfReconciliationCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<LegacySelfReconciliationResult, FundingError> {
    let t = &command.target;
    let keep = t.action == Action::KeepUnsold;
    let state = if keep { "satisfied" } else { "pending" };
    let version: i64 = sqlx::query_scalar(
        "UPDATE public.settlement_obligation_fulfillments SET execution_state=$3,
          state_version=state_version+1,claim_token=NULL,claimed_at=NULL,claim_expires_at=NULL,
          next_attempt_at=NULL,last_error_code=NULL,
          completed_at=CASE WHEN $4 THEN clock_timestamp() ELSE NULL END,
          proof_kind=CASE WHEN $4 THEN 'bot_legacy_self_v1' ELSE NULL END,
          proof_reference=CASE WHEN $4 THEN $5::text ELSE NULL END,
          proof_digest=CASE WHEN $4 THEN $6::bytea ELSE NULL END,
          proof_observed_at=CASE WHEN $4 THEN $7::timestamptz ELSE NULL END,
          delivery_id=CASE WHEN $4 THEN delivery_id ELSE NULL END,
          prepare_command_id=CASE WHEN $4 THEN prepare_command_id ELSE NULL END,
          prepare_request_digest=CASE WHEN $4 THEN prepare_request_digest ELSE NULL END,
          activate_command_id=CASE WHEN $4 THEN activate_command_id ELSE NULL END,
          activate_request_digest=CASE WHEN $4 THEN activate_request_digest ELSE NULL END,
          updated_at=clock_timestamp()
         WHERE obligation_id=$1 AND execution_state='manual_reconcile' AND state_version=$2
           AND claim_token IS NULL AND proof_kind IS NULL RETURNING state_version",
    )
    .bind(t.obligation_id)
    .bind(t.expected_fulfillment_version)
    .bind(state)
    .bind(keep)
    .bind(command.command_id.to_string())
    .bind(evidence.digest.to_vec())
    .bind(evidence.observed_from)
    .fetch_one(&mut **tx)
    .await
    .map_err(sql_error)?;
    Ok(LegacySelfReconciliationResult {
        command_id: command.command_id,
        delivery_id: t.delivery_id,
        obligation_id: t.obligation_id,
        action: t.action,
        fulfillment_state: state.to_owned(),
        fulfillment_version: version,
    })
}

fn evidence_summary(e: &SettlementInternalEvidenceSnapshot) -> Value {
    let binding = |b: &auction_application::MailAccountEvidenceBinding| {
        json!({
        "credential_id":b.credential_id,"credential_version":b.credential_version,
        "runtime":{"generation":b.runtime.generation},"authenticated_uid":b.authenticated_uid,
        "player_name":b.player_name,"world":b.world,"observed_at":b.observed_at,
        "protocol_version":b.protocol_version,"digest":b.digest})
    };
    let purpose = match &e.purpose {
        InternalEvidencePurpose::LegacySelfReconciliation {
            command_id,
            obligation_id,
            expected_fulfillment_version,
        } => {
            json!({"LegacySelfReconciliation":{"command_id":command_id,"obligation_id":obligation_id,
            "expected_fulfillment_version":expected_fulfillment_version}})
        }
        _ => Value::Null,
    };
    json!({"identity":{"uid":e.identity.authenticated_uid,"player_name":e.identity.player_name},
        "full_snapshot":{"operation_id":e.operation_id,"purpose":purpose,"identity":binding(&e.identity),
        "equipment":e.equipment.as_ref().map(|p|json!({"binding":binding(&p.binding),
            "equipment_id":p.equipment_id,"external_equipment_id":p.external_equipment_id,
            "category":p.category,"observed_at":p.observed_at,"digest":p.digest})),
        "main_cash":null,"sent_collection":null,
        "observed_from":e.observed_from,"expires_at":e.expires_at,"digest":e.digest}})
}
