//! MoogleMail 入站事实与邮件命令账本的幂等写入。

use crate::database::storage_error_from;

use auction_application::core::ApplicationError;
use auction_application::mail::MailPollIntake;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Row, types::Json};
use uuid::Uuid;

use super::storage_error;

pub(crate) async fn lookup_completion(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    mailbox_key: &str,
    world_id: &str,
    external_mail_id: &str,
    received_at: DateTime<Utc>,
) -> Result<auction_application::mail::MailPollCompletion, ApplicationError> {
    let row = sqlx::query(
        "SELECT inbound.id, inbound.world_id, inbound.mailbox_key,
                inbound.received_at, inbound.content_digest, inbound.status,
                inbound.decision_code,
                command.command_id, command.mail_id, command.request_digest,
                command.result_payload
         FROM public.inbound_mail AS inbound
         LEFT JOIN public.mail_commands AS command ON command.mail_id = inbound.id
         WHERE inbound.mailbox_key = $1 AND inbound.external_mail_id = $2",
    )
    .bind(mailbox_key)
    .bind(external_mail_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 MoogleMail 已完成事实失败"))?;
    let Some(row) = row else {
        return Ok(auction_application::mail::MailPollCompletion::Missing);
    };
    let inbound_id: Uuid = row
        .try_get("id")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完成事实 ID 失败"))?;
    let stored_mailbox: String = row
        .try_get("mailbox_key")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完成事实邮箱失败"))?;
    let stored_world: String = row
        .try_get("world_id")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完成事实世界失败"))?;
    let stored_received: DateTime<Utc> = row
        .try_get("received_at")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完成事实时间失败"))?;
    let stored_digest: Vec<u8> = row
        .try_get("content_digest")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完成事实摘要失败"))?;
    let stored_status: String = row
        .try_get("status")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完成事实状态失败"))?;
    let stored_code: String = row
        .try_get("decision_code")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完成事实决定码失败"))?;
    if stored_mailbox != mailbox_key
        || stored_world != world_id
        || stored_received.timestamp_micros() != received_at.timestamp_micros()
    {
        return Err(storage_error("MoogleMail 完成事实身份冲突"));
    }
    let expected_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/inbound-mail-v1/{mailbox_key}/{world_id}/{external_mail_id}").as_bytes(),
    );
    let command_id: Option<Uuid> = row
        .try_get("command_id")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完成命令 ID 失败"))?;
    let mail_id: Option<Uuid> = row
        .try_get("mail_id")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完成命令邮件 ID 失败"))?;
    let request_digest: Option<Vec<u8>> = row
        .try_get("request_digest")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完成命令摘要失败"))?;
    let result_payload: Option<Json<Value>> = row
        .try_get("result_payload")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完成命令结果失败"))?;
    let expected_command = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/mail-intake-command-v1/{mailbox_key}/{external_mail_id}").as_bytes(),
    );
    if inbound_id != expected_id
        || command_id != Some(expected_command)
        || mail_id != Some(inbound_id)
        || stored_digest.len() != 32
        || stored_digest.iter().all(|byte| *byte == 0)
        || !matches!(stored_status.as_str(), "accepted" | "rejected" | "failed")
        || request_digest.as_ref() != Some(&stored_digest)
        || result_payload.as_ref().is_none_or(|payload| {
            payload.0
                != serde_json::json!({
                    "mail_id": inbound_id,
                    "status": stored_status,
                    "decision_code": stored_code,
                })
        })
        || stored_code == "parsed_pending_processing"
    {
        return Err(storage_error("MoogleMail 完成事实或命令账本损坏"));
    }
    Ok(auction_application::mail::MailPollCompletion::Completed)
}

pub(crate) async fn insert_or_verify_intake(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    mailbox_key: &str,
    item: &MailPollIntake,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at,
             sender, subject, message_display, equipment_diagnostic)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
         ON CONFLICT (mailbox_key, external_mail_id) DO NOTHING",
    )
    .bind(item.id)
    .bind(mailbox_key)
    .bind(&item.external_mail_id)
    .bind(&item.world_id)
    .bind(&item.mail_kind)
    .bind(item.content_digest.as_slice())
    .bind(item.received_at)
    .bind(&item.status)
    .bind(&item.decision_code)
    .bind(Json(&item.decision_metadata))
    .bind(item.processed_at)
    .bind(&item.sender)
    .bind(&item.subject)
    .bind(item.message_display.as_ref().map(Json))
    .bind(item.equipment_diagnostic.as_ref().map(Json))
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入 MoogleMail 入站事实失败"))?;

    let row = sqlx::query(
        "SELECT id, world_id, mail_kind, content_digest, received_at, status,
                decision_code, decision_metadata
         FROM public.inbound_mail
         WHERE mailbox_key = $1 AND external_mail_id = $2
         FOR UPDATE",
    )
    .bind(mailbox_key)
    .bind(&item.external_mail_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "校验 MoogleMail 入站幂等事实失败"))?;
    let stored_id: Uuid = row
        .try_get("id")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 入站 ID 失败"))?;
    let stored_world: String = row
        .try_get("world_id")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 入站世界失败"))?;
    let stored_kind: String = row
        .try_get("mail_kind")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 入站类型失败"))?;
    let stored_digest: Vec<u8> = row
        .try_get("content_digest")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 入站摘要失败"))?;
    let stored_received: DateTime<Utc> = row
        .try_get("received_at")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 入站时间失败"))?;
    let stored_status: String = row
        .try_get("status")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 入站状态失败"))?;
    let stored_code: String = row
        .try_get("decision_code")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 入站决定码失败"))?;
    let stored_metadata: Json<Value> = row
        .try_get("decision_metadata")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 入站 metadata 失败"))?;
    // `processed_at` 保留首次入站轮询写入的时间值，并非远端邮件事实。PostgreSQL 会把 Rust
    // 纳秒时间规范到微秒；received_at 按相同存储精度比较，processed_at 不参与重放比较或覆盖。
    if stored_id != item.id
        || stored_world != item.world_id
        || stored_kind != item.mail_kind
        || stored_digest != item.content_digest
        || stored_received.timestamp_micros() != item.received_at.timestamp_micros()
        || stored_status != item.status
        || stored_code != item.decision_code
        || stored_metadata.0 != item.decision_metadata
    {
        return Err(storage_error("MoogleMail 外部邮件 ID 对应内容发生冲突"));
    }
    Ok(())
}

pub(crate) async fn insert_or_verify_mail_command(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    mailbox_key: &str,
    item: &MailPollIntake,
) -> Result<(), ApplicationError> {
    let command_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/mail-intake-command-v1/{mailbox_key}/{}", item.external_mail_id)
            .as_bytes(),
    );
    let result_payload = serde_json::json!({
        "mail_id": item.id,
        "status": item.status,
        "decision_code": item.decision_code,
    });
    sqlx::query(
        "INSERT INTO public.mail_commands
            (command_id, request_digest, mail_id, result_payload)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (command_id) DO NOTHING",
    )
    .bind(command_id)
    .bind(item.content_digest.as_slice())
    .bind(item.id)
    .bind(Json(&result_payload))
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入 MoogleMail 命令账本失败"))?;
    let row = sqlx::query(
        "SELECT request_digest, mail_id, result_payload
         FROM public.mail_commands WHERE command_id = $1",
    )
    .bind(command_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 MoogleMail 命令账本失败"))?;
    let request_digest: Vec<u8> = row
        .try_get("request_digest")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 命令摘要失败"))?;
    let mail_id: Uuid = row
        .try_get("mail_id")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 命令邮件 ID 失败"))?;
    let stored_payload: Json<Value> = row
        .try_get("result_payload")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 命令结果失败"))?;
    if request_digest != item.content_digest
        || mail_id != item.id
        || stored_payload.0 != result_payload
    {
        return Err(storage_error("MoogleMail 命令账本发生请求冲突"));
    }
    Ok(())
}
