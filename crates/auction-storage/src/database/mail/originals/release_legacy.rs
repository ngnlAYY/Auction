//! 隔离交还原拍卖 owner 前补写旧格式入站行、命令账本与原信链接。

use auction_application::consignment::DepositBusinessRejection;
use auction_application::settlement::SYSTEM_RETURN_SENDER;
use auction_domain::consignment::deposit::{EnvelopeItemKind, LegacyReturnEvidence};
use auction_domain::consignment::settlement_world_code;
use auction_domain::valid_text;
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::identity::StoredQuarantine;
use super::observations::StoredObservation;
use super::{conflict, invariant};
use crate::database::consignment::commands::errors::WriteError;

/// 交还旧 owner 前补写旧格式 `inbound_mail` 兼容行、命令账本与 K 链接。
///
/// 裁定：旧证据以观察派生事实保存，交接时在同一事务内按旧公式与旧字段集补齐；同键已存在
/// 时逐字段核对后复用，任何不一致都失败关闭，不覆盖历史行。
pub(super) async fn ensure_legacy_inbound(
    connection: &mut PgConnection,
    quarantine: &StoredQuarantine,
    observation: &StoredObservation,
    evidence: &LegacyReturnEvidence,
    now: Timestamp,
) -> Result<Uuid, WriteError> {
    let key = &quarantine.mail_key;
    let external_mid = key.external_mid();
    let world_id = settlement_world_code(key.world());
    // 旧消费者的身份键只有 (world, external_mid) 与 mailbox 完成键。先按物理邮件找既有退件行；
    // 同世界同 MID 出现多条旧退件时无法确定原案，失败关闭而不是任选一条。
    let existing = sqlx::query(
        "SELECT id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
                received_at, status, decision_code, decision_metadata, sender, subject
         FROM public.inbound_mail
         WHERE world_id = $1 AND external_mail_id = $2
           AND mail_kind = 'delivery' AND status = 'accepted'
         ORDER BY created_at, id
         FOR UPDATE",
    )
    .bind(world_id)
    .bind(external_mid)
    .fetch_all(&mut *connection)
    .await?;
    if existing.len() > 1 {
        return Err(invariant("consignment_release_legacy_inbound_ambiguous"));
    }

    // 观察派生事实完整时才校验旧证据；只有身份、没有旧证据键的既有行仍可复用，但新建行
    // 必须从观察事实重建完整旧格式，不能伪造旧证据。
    let strict_metadata = if evidence.obligation_id.is_some() || evidence.subject.is_some() {
        if evidence.subject.is_none() {
            return Err(invariant("consignment_release_legacy_subject"));
        }
        Some(legacy_decision_metadata(observation, evidence)?)
    } else {
        None
    };
    let strict_subject = evidence.subject.as_deref();

    let (inbound_id, mailbox_key, content_digest, stored_status, stored_code) = if let Some(row) =
        existing.first()
    {
        verify_legacy_inbound(
            row,
            world_id,
            external_mid,
            strict_metadata.as_ref(),
            strict_subject,
        )?;
        let inbound_id = sqlx::Row::try_get::<Uuid, _>(row, "id")?;
        let mailbox_key = sqlx::Row::try_get::<String, _>(row, "mailbox_key")?;
        let content_digest = sqlx::Row::try_get::<Vec<u8>, _>(row, "content_digest")?;
        let stored_status = sqlx::Row::try_get::<String, _>(row, "status")?;
        let stored_code = sqlx::Row::try_get::<String, _>(row, "decision_code")?;
        (inbound_id, mailbox_key, content_digest, stored_status, stored_code)
    } else {
        let metadata = strict_metadata
            .as_ref()
            .ok_or_else(|| invariant("consignment_release_legacy_evidence"))?;
        let subject =
            strict_subject.ok_or_else(|| invariant("consignment_release_legacy_subject"))?;
        let mailbox_key = observation.mailbox_key.clone();
        let inbound_id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("auction/inbound-mail-v1/{mailbox_key}/{world_id}/{external_mid}").as_bytes(),
        );
        let content_digest = observation.manifest_digest.to_vec();
        let processed_at =
            if observation.received_at > now { observation.received_at } else { now };
        sqlx::query(
            "INSERT INTO public.inbound_mail
                (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
                 received_at, status, decision_code, decision_metadata, processed_at,
                 sender, subject, message_display, equipment_diagnostic)
             VALUES ($1, $2, $3, $4, 'delivery', $5, $6, 'accepted',
                     'settlement_recovery_returned', $7, $8, $9, $10, NULL, NULL)
             ON CONFLICT (mailbox_key, external_mail_id) DO NOTHING",
        )
        .bind(inbound_id)
        .bind(&mailbox_key)
        .bind(external_mid)
        .bind(world_id)
        .bind(content_digest.as_slice())
        .bind(observation.received_at)
        .bind(metadata)
        .bind(processed_at)
        .bind(SYSTEM_RETURN_SENDER)
        .bind(subject)
        .execute(&mut *connection)
        .await?;
        let row = sqlx::query(
            "SELECT id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
                    received_at, status, decision_code, decision_metadata, sender, subject
             FROM public.inbound_mail
             WHERE mailbox_key = $1 AND external_mail_id = $2
             FOR UPDATE",
        )
        .bind(&mailbox_key)
        .bind(external_mid)
        .fetch_one(&mut *connection)
        .await?;
        verify_legacy_inbound(&row, world_id, external_mid, Some(metadata), Some(subject))?;
        if sqlx::Row::try_get::<Uuid, _>(&row, "id")? != inbound_id {
            return Err(invariant("consignment_release_legacy_inbound_identity"));
        }
        (
            inbound_id,
            mailbox_key,
            content_digest,
            "accepted".to_owned(),
            "settlement_recovery_returned".to_owned(),
        )
    };

    sqlx::query(
        "INSERT INTO public.mail_original_links
            (inbound_id, mailbox_key, external_mid, bot_uid, world_id, auth_observation_id)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT DO NOTHING",
    )
    .bind(inbound_id)
    .bind(&mailbox_key)
    .bind(external_mid)
    .bind(key.authenticated_bot_uid())
    .bind(world_id)
    .bind(observation.observation_id)
    .execute(&mut *connection)
    .await?;
    let link = sqlx::query(
        "SELECT inbound_id, mailbox_key, bot_uid, world_id, external_mid
         FROM public.mail_original_links
         WHERE inbound_id = $1",
    )
    .bind(inbound_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| invariant("consignment_release_legacy_link"))?;
    if sqlx::Row::try_get::<Uuid, _>(&link, "inbound_id")? != inbound_id
        || sqlx::Row::try_get::<String, _>(&link, "mailbox_key")? != mailbox_key
        || sqlx::Row::try_get::<String, _>(&link, "bot_uid")? != key.authenticated_bot_uid()
        || sqlx::Row::try_get::<String, _>(&link, "world_id")? != world_id
        || sqlx::Row::try_get::<String, _>(&link, "external_mid")? != external_mid
    {
        return Err(invariant("consignment_release_legacy_link_conflict"));
    }

    // 兼容旧 `lookup_completion`：存在同键入站行时，命令账本必须给出一致的完成事实，
    // 否则下一次轮询会把补齐行误判为损坏。
    let command_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/mail-intake-command-v1/{mailbox_key}/{external_mid}").as_bytes(),
    );
    let result_payload = json!({
        "mail_id": inbound_id,
        "status": stored_status,
        "decision_code": stored_code,
    });
    sqlx::query(
        "INSERT INTO public.mail_commands
            (command_id, request_digest, mail_id, result_payload)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT DO NOTHING",
    )
    .bind(command_id)
    .bind(content_digest.as_slice())
    .bind(inbound_id)
    .bind(&result_payload)
    .execute(&mut *connection)
    .await?;
    let command = sqlx::query(
        "SELECT request_digest, mail_id, result_payload
         FROM public.mail_commands WHERE command_id = $1",
    )
    .bind(command_id)
    .fetch_one(&mut *connection)
    .await?;
    let command_digest: Vec<u8> = sqlx::Row::try_get(&command, "request_digest")?;
    let command_payload: Value = sqlx::Row::try_get(&command, "result_payload")?;
    if command_digest.as_slice() != content_digest.as_slice()
        || sqlx::Row::try_get::<Uuid, _>(&command, "mail_id")? != inbound_id
        || command_payload != result_payload
    {
        return Err(invariant("consignment_release_legacy_command_conflict"));
    }
    Ok(inbound_id)
}

fn verify_legacy_inbound(
    row: &sqlx::postgres::PgRow,
    world_id: &str,
    external_mid: &str,
    metadata: Option<&Value>,
    subject: Option<&str>,
) -> Result<(), WriteError> {
    let stored_metadata: Value = sqlx::Row::try_get(row, "decision_metadata")?;
    let stored_subject: Option<String> = sqlx::Row::try_get(row, "subject")?;
    // 复用不强制旧决定码：隔离后才交还时，既有旧行可能已被标成 `suspected_system_rts`。
    // 世界、MID、发件人、主题与证据仍须一致，且绝不覆盖历史行。
    if sqlx::Row::try_get::<String, _>(row, "world_id")? != world_id
        || sqlx::Row::try_get::<String, _>(row, "external_mail_id")? != external_mid
        || sqlx::Row::try_get::<String, _>(row, "mail_kind")? != "delivery"
        || sqlx::Row::try_get::<String, _>(row, "status")? != "accepted"
        || sqlx::Row::try_get::<Option<String>, _>(row, "sender")?.as_deref()
            != Some(SYSTEM_RETURN_SENDER)
        || metadata.is_some_and(|expected| stored_metadata != *expected)
        || subject.is_some_and(|expected| stored_subject.as_deref() != Some(expected))
    {
        return Err(invariant("consignment_release_legacy_inbound_conflict"));
    }
    Ok(())
}

/// 旧路径写入的严格退件证据；与 `SettlementReturnEvidence` 的字段集逐键一致。
fn legacy_decision_metadata(
    observation: &StoredObservation,
    evidence: &LegacyReturnEvidence,
) -> Result<Value, WriteError> {
    let obligation_id =
        evidence.obligation_id.ok_or_else(|| invariant("consignment_release_legacy_obligation"))?;
    let credits = observation
        .credits
        .filter(|value| *value >= 0)
        .ok_or_else(|| invariant("consignment_release_legacy_credits"))?;
    let cod = observation
        .cod
        .filter(|value| *value >= 0)
        .ok_or_else(|| invariant("consignment_release_legacy_cod"))?;
    let mut attachment_ids = observation
        .items
        .iter()
        .filter(|item| item.kind == EnvelopeItemKind::Equipment)
        .map(|item| {
            item.raw_eid
                .clone()
                .filter(|value| valid_text(value, 128))
                .ok_or_else(|| invariant("consignment_release_legacy_attachment"))
        })
        .collect::<Result<Vec<_>, WriteError>>()?;
    attachment_ids.sort_unstable();
    if attachment_ids.len() > 100
        || attachment_ids.windows(2).any(|pair| pair[0].as_str() >= pair[1].as_str())
    {
        return Err(conflict(DepositBusinessRejection::EvidenceConflict));
    }
    let mut metadata = json!({
        "obligation_id": obligation_id.to_string(),
        "credits": credits,
        "cod": cod,
        "attachment_ids": attachment_ids,
    });
    if let Some(returned_from) = &evidence.returned_from {
        metadata["returned_from"] = json!(returned_from);
    }
    if let Some(body_digest) = evidence.body_digest {
        metadata["body_digest"] =
            json!(body_digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>());
    }
    if let Some(reference) = &evidence.dispatch_reference {
        metadata["dispatch_reference"] = json!(reference);
    }
    Ok(metadata)
}
