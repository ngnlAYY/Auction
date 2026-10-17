//! PostgreSQL 普通 delivery 入队、幂等复读和版本化 payload。

use auction_application::{
    MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin, MailDeliveryResult,
    RepositoryError,
};
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
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    if command.origin == MailDeliveryOrigin::MailIntake
        && command.operation != MailDeliveryOperation::Send
    {
        // 入站录入在 Serializable 快照内读取相反意图；处置事务也必须参与 SSI，单独
        // SELECT FOR UPDATE 的世界行锁不能刷新等待方已建立的快照。40001 由现有邮件
        // 业务有界重试重新执行整次事务，不能按旧快照继续提交。
        sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
            .execute(&mut *transaction)
            .await
            .map_err(map_storage_error)?;
    }
    let result = enqueue_in_transaction(&mut transaction, command, prepared_status).await?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

/// 由调用方持有权威业务事务；delivery SQL 与审计仍只有本 owner 写入。
pub(crate) async fn enqueue_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &MailDeliveryCommand,
    prepared_status: Option<&str>,
) -> Result<MailDeliveryResult, RepositoryError> {
    super::validation::validate_command(command)
        .map_err(|_| repository_error("MoogleMail delivery 命令无效"))?;
    let payload = payload(command);
    let status = prepared_status.unwrap_or("pending");
    let occurred_at = Utc::now();
    let delivery_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/mail-delivery-v1/{}", command.command_id).as_bytes(),
    );
    super::disposition::guard_disposition(transaction, command).await?;
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
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;

    let row = sqlx::query(
        "SELECT id, request_digest, origin_kind, origin_id, recipient, payload, status
         FROM public.deliveries
         WHERE command_id = $1 FOR UPDATE",
    )
    .bind(command.command_id.as_uuid())
    .fetch_optional(&mut **transaction)
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
        || !matching_origin_id(&stored_origin_id, command)
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
            transaction,
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
    Ok(MailDeliveryResult { delivery_id: stored_id })
}

pub(crate) fn payload(command: &MailDeliveryCommand) -> Value {
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

fn matching_origin_id(stored: &str, command: &MailDeliveryCommand) -> bool {
    if stored == command.origin_id {
        return true;
    }
    if command.origin.as_str() != "mail_intake" {
        return false;
    }
    let canonical =
        format!("{}:{}:{}", command.world_id, command.external_mail_id, command.operation.as_str());
    let legacy_operation = match command.operation.as_str() {
        "send" => "delivery-notice",
        "return" => "delivery-return",
        "take" => "delivery-take",
        _ => return false,
    };
    let legacy = format!("{}:{}:{legacy_operation}", command.world_id, command.external_mail_id);
    // 请求摘要、收件人和完整 payload 仍由调用处逐项比较；这里只允许同一 mail_intake
    // command 的已知历史后缀与规范 operation 后缀互认。
    matches!(stored, value if value == canonical || value == legacy)
        && matches!(command.origin_id.as_str(), value if value == canonical || value == legacy)
}
