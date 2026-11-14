//! 只读恢复旧 prepare 已提交、action 尚未关联的升级窗口；不创建或改写投递。

use auction_application::core::RepositoryError;
use auction_application::mail::{MailDeliveryCommand, MailDeliveryResult};
use sqlx::Row;

use super::errors::{map_storage_error, repository_error};

pub(super) async fn find_postgres(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    command: &MailDeliveryCommand,
) -> Result<Option<MailDeliveryResult>, RepositoryError> {
    let row = sqlx::query(
        "SELECT id, request_digest, origin_kind, origin_id, recipient, payload, status
         FROM public.deliveries WHERE command_id = $1",
    )
    .bind(command.command_id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(map_storage_error)?;
    let Some(row) = row else { return Ok(None) };
    let payload: serde_json::Value = row.try_get("payload").map_err(map_storage_error)?;
    if row.try_get::<String, _>("origin_kind").map_err(map_storage_error)?
        != command.origin.as_str()
        || row.try_get::<String, _>("origin_id").map_err(map_storage_error)? != command.origin_id
        || payload["world"].as_str() != Some(command.world_id.as_str())
        || payload["operation"].as_str() != Some("take")
    {
        return Err(repository_error("旧 recovery prepare 来源冲突"));
    }
    let source_mail = payload["external_mail_id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| repository_error("旧 recovery prepare 邮件身份无效"))?;
    if source_mail != command.external_mail_id {
        // 同一 action 的旧版 ID 可能属于上一封退件；新 MID 才能获得新的轮次身份。
        return Ok(None);
    }
    if row.try_get::<Vec<u8>, _>("request_digest").map_err(map_storage_error)?
        != command.request_hash.as_bytes()
        || row.try_get::<String, _>("recipient").map_err(map_storage_error)? != command.recipient
        || payload != super::outbox::payload(command)
        || !matches!(
            row.try_get::<String, _>("status").map_err(map_storage_error)?.as_str(),
            "prepared" | "pending"
        )
    {
        return Err(repository_error("旧 recovery prepare 完整身份或状态冲突"));
    }
    Ok(Some(MailDeliveryResult { delivery_id: row.try_get("id").map_err(map_storage_error)? }))
}
