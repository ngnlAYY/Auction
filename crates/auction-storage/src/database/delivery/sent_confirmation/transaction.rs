//! 追加命令事实、唯一 MID 与 succeeded 投递；原始未知结果或被更正的 Admin 失败结论保持在
//! 不可变快照、原命令账本与旧审计中。

use auction_application::delivery::{SentConfirmationCommand, SentConfirmationResult};
use serde_json::json;
use sqlx::{Postgres, Row};

use crate::database::delivery::scheduler::attempts;
use crate::database::delivery::sent_confirmation::{Error, snapshot};
use crate::database::{TrackedTransaction as Transaction, settlement::funding};

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
    let admission = snapshot.admission()?;
    let obligation_id = admission.obligation.as_ref().map(|o| o.id).ok_or(Error::Rejected)?;
    // 准入已要求结算载荷可投影；这里仍失败关闭，不对缺失载荷做任何补全。
    let template = admission
        .delivery
        .payload
        .as_ref()
        .map(|payload| payload.template.clone())
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
    let corrects_admin_failure = p.corrects_admin_failure;
    let corrected_admin_failure_command_id = corrects_admin_failure
        .then(|| admission.admin_failure.as_ref().map(|failure| failure.command_id))
        .flatten();
    // 不设置 sent_verified；下游只能通过持久 confirmation+attempt+binding 强谓词获得此证明能力。
    let mut metadata = json!({
        "operation":"send", "proof":"sent_operator_confirmed_v1",
        "external_reference_proven":true, "operator_confirmation_id":command.command_id,
        "send_expectation":expected, "sending_fencing_version":snapshot.sending_fencing_version,
        "recipient":command.evidence.recipient,
        "template":template,
        "settlement_obligation_id":obligation_id,
        "sent_at":command.evidence.sent_minute,
        "sent_time_precision":"minute", "collected":command.evidence.collected,
        "cod_pending":command.evidence.cod_pending,
    });
    if corrects_admin_failure {
        metadata["corrected_admin_failure_command_id"] = json!(corrected_admin_failure_command_id);
    }
    // 状态门与快照准入一致：更正只作用于仍停在原 Admin 失败结论上的行，失败标记随成功移除，
    // 原结论保留在命令账本、审计与 confirmation 快照中。
    let changed = sqlx::query("UPDATE public.deliveries SET status='succeeded',external_reference=$3,
        failure_code=NULL,completed_at=clock_timestamp(),updated_at=clock_timestamp(),
        result_metadata=(result_metadata-CASE WHEN $5 THEN ARRAY['sent_verified','sent_recovered','resolution']
                                              ELSE ARRAY['sent_verified','sent_recovered'] END::text[]) || $4::jsonb
        WHERE id=$1 AND fencing_version=$2
          AND external_reference IS NULL AND lease_token IS NULL AND lease_expires_at IS NULL
          AND CASE WHEN $5 THEN status='failed'
                   AND result_metadata->>'resolution'='admin_reconciled_failed'
              ELSE status='manual_reconcile' AND failure_code='write_unknown' END")
        .bind(command.delivery_id).bind(command.expected_fencing_version)
        .bind(&result.external_reference).bind(metadata).bind(corrects_admin_failure)
        .execute(&mut **tx).await?;
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
            auction_application::settlement::FundingDeliveryOutcome::Sent,
        )
        .await?;
    }
    crate::database::mail::mailbox_sweep::resume_settlement_send(
        tx,
        obligation_id,
        command.delivery_id,
        if corrects_admin_failure { "failed" } else { "manual_reconcile" },
    )
    .await?;
    sqlx::query("INSERT INTO public.audit_records(id,occurred_at,actor_type,actor_id,action,entity_type,
        entity_id,result,source,metadata) VALUES($1,clock_timestamp(),'admin',$2,'delivery.sent_operator_confirmed',
        'delivery',$3,'success',$5,$4)")
        .bind(command.command_id).bind(&command.actor).bind(command.delivery_id.to_string())
        .bind(auction_application::core::cli_audit_metadata(command.operator_identity.as_ref(), json!({"command_id":command.command_id,"request_digest":digest,
            "fencing_version":command.expected_fencing_version,
            "sending_fencing_version":snapshot.sending_fencing_version,
            "external_reference":result.external_reference,
            "accept_collected_assets":command.accept_collected_assets,
            "accept_attempt_history":command.accept_attempt_history,
            "corrects_admin_failure":corrects_admin_failure,
            "admin_failure_command_id":corrected_admin_failure_command_id,
            "proof":"sent_operator_confirmed_v1","executor":"auction-cli",
            "reason":command.reason,
            "collection_funding":if expected.credits > 0 {"already_collected"} else {"normal_worker"},
            "seller_payment":if expected.credits > 0 {"original_sent_confirmed"} else {"normal_queue"},
            "funding_allocation_id":admission.funding_allocation.as_ref().map(|allocation| allocation.id)})))
        .bind(if command.operator_identity.is_some() { "cli" } else { "admin" })
        .execute(&mut **tx).await?;
    // 同一事务追加统一处置事实：人工证明解除了原未知或已被判失败的终态。
    crate::database::delivery::disposition::append(
        tx,
        crate::database::delivery::disposition::DeliveryDispositionFact {
            delivery_id: command.delivery_id,
            fencing_version: command.expected_fencing_version,
            kind: auction_domain::DeliveryDispositionKind::OperatorSentConfirmation,
            source: auction_domain::DeliveryDispositionSource {
                sent_confirmation_id: Some(command.command_id),
                ..Default::default()
            },
            actor_source: if command.operator_identity.is_some() {
                auction_domain::DeliveryDispositionActorSource::Cli
            } else {
                auction_domain::DeliveryDispositionActorSource::Admin
            },
            actor_id: &command.actor,
            reason: Some(&command.reason),
            reason_code: Some("sent_operator_confirmed_v1"),
            detail: Some(&json!({
                "proof": "sent_operator_confirmed_v1",
                "corrects_admin_failure": corrects_admin_failure,
                "sending_fencing_version": snapshot.sending_fencing_version,
            })),
            command_id: None,
            request_digest: None,
        },
    )
    .await?;
    Ok(result)
}
