//! MoogleMail 入站事实与邮件命令账本的幂等写入。

use auction_application::{ApplicationError, MailPollIntake};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Row, types::Json};
use uuid::Uuid;

use super::storage_error;

pub(crate) async fn insert_or_verify_intake(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    mailbox_key: &str,
    item: &MailPollIntake,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         ON CONFLICT (mailbox_key, external_mail_id) DO NOTHING",
    )
    .bind(item.id)
    .bind(mailbox_key)
    .bind(&item.external_mail_id)
    .bind(&item.world_id)
    .bind(&item.mail_kind)
    .bind(item.content_digest.to_vec())
    .bind(item.received_at)
    .bind(&item.status)
    .bind(&item.decision_code)
    .bind(Json(item.decision_metadata.clone()))
    .bind(item.processed_at)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("写入 MoogleMail 入站事实失败"))?;

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
    .map_err(|_| storage_error("校验 MoogleMail 入站幂等事实失败"))?;
    let stored_id: Uuid =
        row.try_get("id").map_err(|_| storage_error("读取 MoogleMail 入站 ID 失败"))?;
    let stored_world: String =
        row.try_get("world_id").map_err(|_| storage_error("读取 MoogleMail 入站世界失败"))?;
    let stored_kind: String =
        row.try_get("mail_kind").map_err(|_| storage_error("读取 MoogleMail 入站类型失败"))?;
    let stored_digest: Vec<u8> =
        row.try_get("content_digest").map_err(|_| storage_error("读取 MoogleMail 入站摘要失败"))?;
    let stored_received: DateTime<Utc> =
        row.try_get("received_at").map_err(|_| storage_error("读取 MoogleMail 入站时间失败"))?;
    let stored_status: String =
        row.try_get("status").map_err(|_| storage_error("读取 MoogleMail 入站状态失败"))?;
    let stored_code: String = row
        .try_get("decision_code")
        .map_err(|_| storage_error("读取 MoogleMail 入站决定码失败"))?;
    let stored_metadata: Json<Value> = row
        .try_get("decision_metadata")
        .map_err(|_| storage_error("读取 MoogleMail 入站 metadata 失败"))?;
    // `processed_at` 保留首次入站轮询写入的时间值，并非远端邮件事实。PostgreSQL 会把 Rust
    // 纳秒时间规范到微秒，后续重放也会产生新的本轮时间，因此既不能参与冲突比较，也不能覆盖原值。
    if stored_id != item.id
        || stored_world != item.world_id
        || stored_kind != item.mail_kind
        || stored_digest != item.content_digest
        || stored_received != item.received_at
        || stored_status != item.status
        || stored_code != item.decision_code
        || stored_metadata.0 != item.decision_metadata
    {
        return Err(storage_error("MoogleMail 外部邮件 ID 对应内容发生冲突"));
    }
    Ok(())
}

pub(crate) async fn insert_or_verify_mail_command(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    .bind(item.content_digest.to_vec())
    .bind(item.id)
    .bind(Json(result_payload.clone()))
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("写入 MoogleMail 命令账本失败"))?;
    let row = sqlx::query(
        "SELECT request_digest, mail_id, result_payload
         FROM public.mail_commands WHERE command_id = $1",
    )
    .bind(command_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 MoogleMail 命令账本失败"))?;
    let request_digest: Vec<u8> =
        row.try_get("request_digest").map_err(|_| storage_error("读取 MoogleMail 命令摘要失败"))?;
    let mail_id: Uuid =
        row.try_get("mail_id").map_err(|_| storage_error("读取 MoogleMail 命令邮件 ID 失败"))?;
    let stored_payload: Json<Value> =
        row.try_get("result_payload").map_err(|_| storage_error("读取 MoogleMail 命令结果失败"))?;
    if request_digest != item.content_digest
        || mail_id != item.id
        || stored_payload.0 != result_payload
    {
        return Err(storage_error("MoogleMail 命令账本发生请求冲突"));
    }
    Ok(())
}
