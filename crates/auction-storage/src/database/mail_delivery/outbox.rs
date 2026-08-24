//! PostgreSQL 普通 delivery 入队、幂等复读和版本化 payload。

use auction_application::{MailDeliveryCommand, MailDeliveryResult, RepositoryError};
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::delivery_audit::{
    CREATE, DeliveryAudit, PREPARE, append as append_delivery_audit,
};
use super::common::{map_storage_error, repository_error};

pub(super) async fn enqueue_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    command: &MailDeliveryCommand,
) -> Result<MailDeliveryResult, RepositoryError> {
    insert_postgres(pool, command, None).await
}

pub(super) async fn prepare_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    command: &MailDeliveryCommand,
) -> Result<MailDeliveryResult, RepositoryError> {
    insert_postgres(pool, command, Some("prepared")).await
}

async fn insert_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    command: &MailDeliveryCommand,
    prepared_status: Option<&str>,
) -> Result<MailDeliveryResult, RepositoryError> {
    let payload = payload(command);
    let status = prepared_status.unwrap_or("pending");
    let occurred_at = Utc::now();
    let delivery_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/mail-delivery-v1/{}", command.command_id).as_bytes(),
    );
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let inserted = sqlx::query(
        r#"INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient,
             payload, status, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)
           ON CONFLICT (command_id) DO NOTHING"#,
    )
    .bind(delivery_id)
    .bind(command.command_id.as_uuid())
    .bind(command.request_hash.as_bytes().to_vec())
    .bind(command.origin.as_str())
    .bind(&command.origin_id)
    .bind(&command.recipient)
    .bind(&payload)
    .bind(status)
    .bind(occurred_at)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let row = sqlx::query(
        "SELECT id, request_digest, origin_kind, origin_id, recipient, payload, status
         FROM public.deliveries
         WHERE command_id = $1 FOR UPDATE",
    )
    .bind(command.command_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("MoogleMail delivery 入队结果缺失"))?;
    let stored_digest: Vec<u8> =
        sqlx::Row::try_get(&row, "request_digest").map_err(map_storage_error)?;
    let stored_origin_kind: String =
        sqlx::Row::try_get(&row, "origin_kind").map_err(map_storage_error)?;
    let stored_origin_id: String =
        sqlx::Row::try_get(&row, "origin_id").map_err(map_storage_error)?;
    let stored_recipient: String =
        sqlx::Row::try_get(&row, "recipient").map_err(map_storage_error)?;
    let stored_payload: Value = sqlx::Row::try_get(&row, "payload").map_err(map_storage_error)?;
    let stored_status: String = sqlx::Row::try_get(&row, "status").map_err(map_storage_error)?;
    if stored_digest != command.request_hash.as_bytes()
        || stored_origin_kind != command.origin.as_str()
        || stored_origin_id != command.origin_id
        || stored_recipient != command.recipient
        || stored_payload != payload
        || prepared_status
            .is_some_and(|_| !matches!(stored_status.as_str(), "prepared" | "pending"))
    {
        return Err(repository_error("MoogleMail delivery 命令摘要冲突"));
    }
    let stored_id: Uuid = sqlx::Row::try_get(&row, "id").map_err(map_storage_error)?;
    if inserted.rows_affected() == 1 {
        let request_id = command.command_id.to_string();
        append_delivery_audit(
            &mut transaction,
            DeliveryAudit {
                occurred_at,
                actor_type: "system",
                actor_id: "delivery-service",
                action: if prepared_status.is_some() { PREPARE } else { CREATE },
                delivery_id,
                request_id: Some(&request_id),
                source: "system",
                metadata: json!({
                    "origin_kind": command.origin.as_str(),
                    "status": status,
                }),
            },
        )
        .await
        .map_err(map_storage_error)?;
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(MailDeliveryResult { delivery_id: stored_id })
}

fn payload(command: &MailDeliveryCommand) -> Value {
    json!({
        "schema_version": 1,
        "operation": command.operation.as_str(),
        "world": command.world_id,
        "external_mail_id": command.external_mail_id,
        "template": command.template.map(|template| template.as_str()),
        "result_code": command.result_code,
        "user_key_user_id": command.user_key_user_id,
        "credits": command.credits,
        "cod": command.cod,
        "attachment_ids": command.attachment_ids,
        "body": command.notification_body,
    })
}
