//! 追加命令事实、唯一 MID 与 succeeded 投递；原始未知结果保持在不可变快照与旧审计中。

use super::*;
use serde_json::json;

pub(super) async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    command: &SentConfirmationCommand,
) -> Result<Option<SentConfirmationResult>, Error> {
    let row = sqlx::query(
        "SELECT request_digest,result FROM public.delivery_sent_confirmations WHERE command_id=$1",
    )
    .bind(command.command_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else { return Ok(None) };
    if row.try_get::<Vec<u8>, _>("request_digest")?
        != command.request_digest().map_err(Error::Application)?
    {
        return Err(Error::Rejected);
    }
    let mut result: SentConfirmationResult = serde_json::from_value(row.try_get("result")?)?;
    result.replayed = true;
    Ok(Some(result))
}

pub(super) async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    command: &SentConfirmationCommand,
) -> Result<SentConfirmationResult, Error> {
    // 人工观察不借用当前网络 owner；仍遵循 generation→world→funding→业务→credential→delivery 的锁序。
    sqlx::query("SELECT generation FROM public.mail_runtime_generation WHERE scope='mail_cookie' FOR UPDATE")
        .execute(&mut **tx).await?;
    funding::lock_funding_worlds(tx).await?;
    let account = funding::lock_funding_account(tx, &command.evidence.sender_uid)
        .await?
        .ok_or(Error::Rejected)?;
    if !account.ready() {
        return Err(Error::Rejected);
    }
    for query in [
        "SELECT id FROM public.auctions ORDER BY id FOR UPDATE",
        "SELECT id FROM public.settlement_recoveries ORDER BY id FOR UPDATE",
        "SELECT id FROM public.lots ORDER BY id FOR UPDATE",
        "SELECT id FROM public.equipment ORDER BY id FOR UPDATE",
    ] {
        sqlx::query(query).execute(&mut **tx).await?;
    }
    let initial = snapshot::load(tx, command.delivery_id, &command.evidence).await?;
    let expected = initial.expectation.as_ref().ok_or(Error::Rejected)?;
    if !attempts::lock_credential(tx, expected).await? {
        return Err(Error::Rejected);
    }
    for query in [
        "SELECT id FROM public.users ORDER BY id FOR UPDATE",
        "SELECT id FROM public.settlement_obligations ORDER BY id FOR UPDATE",
        "SELECT id FROM public.settlement_recovery_actions ORDER BY id FOR UPDATE",
        "SELECT id FROM public.deliveries ORDER BY id FOR UPDATE",
        "SELECT obligation_id FROM public.settlement_obligation_fulfillments ORDER BY obligation_id FOR UPDATE",
    ] {
        sqlx::query(query).execute(&mut **tx).await?;
    }
    let snapshot = snapshot::load(tx, command.delivery_id, &command.evidence).await?;
    let p = &snapshot.preview;
    if !p.eligible
        || p.expected_fencing_version != command.expected_fencing_version
        || (p.requires_collected_asset_acceptance && !command.accept_collected_assets)
        || (p.requires_attempt_history_acceptance && !command.accept_attempt_history)
        || snapshot.expectation != initial.expectation
    {
        return Err(Error::Rejected);
    }
    let expected = snapshot.expectation.as_ref().ok_or(Error::Rejected)?;
    let obligation_id = snapshot.data["obligation"]["id"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or(Error::Rejected)?;
    if !attempts::bind_sent(
        tx,
        command.delivery_id,
        snapshot.sending_fencing_version,
        &command.evidence.world_id,
        &command.evidence.external_mail_id,
        true,
    )
    .await?
    {
        return Err(Error::Rejected);
    }
    let digest = command.request_digest().map_err(Error::Application)?;
    let result = SentConfirmationResult {
        command_id: command.command_id,
        delivery_id: command.delivery_id,
        external_reference: p.external_reference.clone(),
        replayed: false,
    };
    sqlx::query("INSERT INTO public.delivery_sent_confirmations(command_id,delivery_id,
        expected_fencing_version,sending_fencing_version,world_id,external_mail_id,request_digest,
        actor,reason,command,evidence,accept_collected_assets,accept_attempt_history,original_snapshot,result)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
        .bind(command.command_id).bind(command.delivery_id).bind(command.expected_fencing_version)
        .bind(snapshot.sending_fencing_version).bind(&command.evidence.world_id)
        .bind(&command.evidence.external_mail_id).bind(digest.as_slice())
        .bind(&command.actor).bind(&command.reason).bind(serde_json::to_value(command)?)
        .bind(serde_json::to_value(&command.evidence)?).bind(command.accept_collected_assets)
        .bind(command.accept_attempt_history).bind(&snapshot.data).bind(serde_json::to_value(&result)?)
        .execute(&mut **tx).await?;
    // 不设置 sent_verified；下游只能通过持久 confirmation+attempt+binding 强谓词获得此证明能力。
    let metadata = json!({
        "operation":"send", "proof":"sent_operator_confirmed_v1",
        "external_reference_proven":true, "operator_confirmation_id":command.command_id,
        "send_expectation":expected, "sending_fencing_version":snapshot.sending_fencing_version,
        "recipient":command.evidence.recipient,
        "template":snapshot.data["delivery"]["payload"]["template"],
        "settlement_obligation_id":obligation_id,
        "sent_at":command.evidence.sent_minute,
        "sent_time_precision":"minute", "collected":command.evidence.collected,
        "cod_pending":command.evidence.cod_pending,
    });
    let changed = sqlx::query("UPDATE public.deliveries SET status='succeeded',external_reference=$3,
        failure_code=NULL,completed_at=clock_timestamp(),updated_at=clock_timestamp(),
        result_metadata=(result_metadata-ARRAY['sent_verified','sent_recovered']::text[]) || $4::jsonb
        WHERE id=$1 AND fencing_version=$2 AND status='manual_reconcile' AND failure_code='write_unknown'
          AND external_reference IS NULL AND lease_token IS NULL AND lease_expires_at IS NULL")
        .bind(command.delivery_id).bind(command.expected_fencing_version)
        .bind(&result.external_reference).bind(metadata).execute(&mut **tx).await?;
    if changed.rows_affected() != 1 {
        return Err(Error::Rejected);
    }
    if expected.credits > 0 {
        // 与自动 recovered-send 使用同一资金 owner：仅把原 unknown 分配记为 spent，
        // 追加原 delivery 身份的支出分录；确认命令重放在 apply 前返回，不会重复扣记。
        funding::finish_funded_delivery(
            tx,
            &account,
            command.delivery_id,
            auction_application::FundingDeliveryOutcome::Sent,
        )
        .await?;
    }
    super::super::mailbox_sweep::resume_settlement_send(tx, obligation_id, command.delivery_id)
        .await?;
    sqlx::query("INSERT INTO public.audit_records(id,occurred_at,actor_type,actor_id,action,entity_type,
        entity_id,result,source,metadata) VALUES($1,clock_timestamp(),'admin',$2,'delivery.sent_operator_confirmed',
        'delivery',$3,'success','admin',$4)")
        .bind(command.command_id).bind(&command.actor).bind(command.delivery_id.to_string())
        .bind(json!({"command_id":command.command_id,"request_digest":digest,
            "fencing_version":command.expected_fencing_version,
            "sending_fencing_version":snapshot.sending_fencing_version,
            "external_reference":result.external_reference,
            "accept_collected_assets":command.accept_collected_assets,
            "accept_attempt_history":command.accept_attempt_history,
            "proof":"sent_operator_confirmed_v1","executor":"auction-cli",
            "reason":command.reason,
            "collection_funding":if expected.credits > 0 {"already_collected"} else {"normal_worker"},
            "seller_payment":if expected.credits > 0 {"original_sent_confirmed"} else {"normal_queue"},
            "funding_allocation_id":snapshot.data["funding_allocation"]["id"]}))
        .execute(&mut **tx).await?;
    Ok(result)
}
