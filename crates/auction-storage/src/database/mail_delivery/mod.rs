//! PostgreSQL MoogleMail delivery outbox 入队适配器。
//!
//! 该 owner 只保存稳定操作、收件人和不含密钥的版本化 payload。真正的远端 POST 仍由 Worker
//! 的 DeliveryTransport 负责；入队成功不等于外部发送成功。

use auction_application::{
    ApplicationError, MailDeliveryActivation, MailDeliveryCommand, MailDeliveryEnqueuer,
    MailDeliveryOperation, MailDeliveryOrigin, MailDeliveryResult, RepositoryError,
};
#[cfg(feature = "postgres")]
use chrono::Utc;
#[cfg(feature = "postgres")]
use serde_json::{Value, json};
use uuid::Uuid;

use super::DatabaseStore;
#[cfg(feature = "postgres")]
use super::delivery_audit::{
    ACTIVATE, CREATE, DeliveryAudit, PREPARE, append as append_delivery_audit,
};

impl MailDeliveryEnqueuer for DatabaseStore {
    fn enqueue(
        &self,
        command: MailDeliveryCommand,
    ) -> auction_application::ApplicationFuture<'_, MailDeliveryResult> {
        Box::pin(async move {
            validate_command(&command)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    enqueue_postgres(store.pool(), &command).await.map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "MoogleMail delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn prepare_recovery(
        &self,
        command: MailDeliveryCommand,
    ) -> auction_application::ApplicationFuture<'_, MailDeliveryResult> {
        Box::pin(async move {
            validate_command(&command)?;
            if command.origin != MailDeliveryOrigin::SettlementRecoveryAction {
                return Err(RepositoryError::Failed(
                    "只有 recovery owner 可以准备两阶段 delivery".to_owned(),
                )
                .into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    prepare_postgres(store.pool(), &command).await.map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "MoogleMail recovery delivery 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn activate_recovery(
        &self,
        activation: MailDeliveryActivation,
    ) -> auction_application::ApplicationFuture<'_, MailDeliveryResult> {
        Box::pin(async move {
            if activation.delivery_id.is_nil()
                || activation.prepare_command_id.is_nil()
                || activation.prepare_request_hash.as_bytes() == [0; 32]
                || activation.activate_command_id.is_nil()
                || activation.activate_request_hash.as_bytes() == [0; 32]
                || activation.activated_at.timestamp_nanos_opt().is_none()
            {
                return Err(RepositoryError::Failed(
                    "recovery delivery activation identity 无效".to_owned(),
                )
                .into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => activate_postgres(store.pool(), &activation)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "MoogleMail recovery delivery 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

fn validate_command(command: &MailDeliveryCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.request_hash.as_bytes() == [0; 32]
        || command.origin_id.is_empty()
        || command.origin_id != command.origin_id.trim()
        || command.origin_id.len() > 512
        || command.origin_id.chars().any(char::is_control)
        || !matches!(command.world_id.as_str(), "main" | "isekai")
        || command.external_mail_id.is_empty()
        || command.external_mail_id != command.external_mail_id.trim()
        || command.recipient.is_empty()
        || command.recipient != command.recipient.trim()
        || command.recipient.chars().any(char::is_control)
        || command.credits < 0
        || command.cod < 0
        || command.attachment_ids.len() > 1000
        || command.attachment_ids.iter().any(|value| {
            value.is_empty()
                || value != value.trim()
                || value.len() > 64
                || !value.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(RepositoryError::Failed("MoogleMail delivery 命令无效".to_owned()).into());
    }
    if !matches!(
        command.origin,
        MailDeliveryOrigin::MailIntake
            | MailDeliveryOrigin::SettlementRecoveryAction
            | MailDeliveryOrigin::OperationsAlert
    ) {
        return Err(RepositoryError::Failed("MoogleMail delivery owner 无效".to_owned()).into());
    }
    if command.origin == MailDeliveryOrigin::SettlementRecoveryAction
        && command
            .origin_id
            .strip_prefix("recovery:")
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_none()
    {
        return Err(
            RepositoryError::Failed("MoogleMail recovery delivery 来源无效".to_owned()).into()
        );
    }
    if command.origin == MailDeliveryOrigin::OperationsAlert
        && (command
            .origin_id
            .strip_prefix("operations_alert:")
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_none()
            || command.notification_body.is_none())
    {
        return Err(RepositoryError::Failed("operations alert delivery 来源无效".to_owned()).into());
    }
    if command.origin != MailDeliveryOrigin::OperationsAlert && command.notification_body.is_some()
    {
        return Err(RepositoryError::Failed(
            "普通 MoogleMail delivery 不得携带通知正文".to_owned(),
        )
        .into());
    }
    if command
        .attachment_ids
        .iter()
        .enumerate()
        .any(|(index, value)| command.attachment_ids[..index].contains(value))
    {
        return Err(RepositoryError::Failed("MoogleMail delivery 附件重复".to_owned()).into());
    }
    match command.operation {
        MailDeliveryOperation::Send => {
            if command.template.is_none()
                || command.result_code.as_deref().is_none_or(str::is_empty)
            {
                return Err(RepositoryError::Failed("MoogleMail 通知模板缺失".to_owned()).into());
            }
            if !command.attachment_ids.is_empty() || command.credits != 0 || command.cod != 0 {
                return Err(RepositoryError::Failed("MoogleMail 通知载荷越界".to_owned()).into());
            }
            match command.template {
                Some(auction_application::MailNotificationTemplate::RegisterKey) => {
                    if command.origin != MailDeliveryOrigin::MailIntake
                        || command.user_key_user_id.is_none()
                        || !matches!(
                            command.result_code.as_deref(),
                            Some("registered" | "recovered")
                        )
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail 注册 key 通知引用无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(auction_application::MailNotificationTemplate::RegisterResult) => {
                    if command.origin != MailDeliveryOrigin::MailIntake
                        || command.user_key_user_id.is_some()
                        || !matches!(
                            command.result_code.as_deref(),
                            Some("blacklisted" | "identity_conflict")
                        )
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail 注册结果通知引用无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(auction_application::MailNotificationTemplate::RebindResult) => {
                    if command.origin != MailDeliveryOrigin::MailIntake
                        || command.user_key_user_id.is_some()
                        || !matches!(
                            command.result_code.as_deref(),
                            Some(
                                "rebound"
                                    | "unchanged"
                                    | "blacklisted"
                                    | "identity_conflict"
                                    | "invalid_key"
                                    | "missing_key"
                            )
                        )
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail 换绑结果通知引用无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(auction_application::MailNotificationTemplate::RecoveryPaymentReminder) => {
                    if command.origin != MailDeliveryOrigin::SettlementRecoveryAction
                        || command.user_key_user_id.is_some()
                        || command.result_code.as_deref() != Some("payment_overdue")
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail recovery 提醒通知引用无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(auction_application::MailNotificationTemplate::OperationsAlert) => {
                    if command.origin != MailDeliveryOrigin::OperationsAlert
                        || command.user_key_user_id.is_some()
                        || command.result_code.as_deref() != Some("operations_alert")
                        || !valid_notification_body(command.notification_body.as_deref())
                    {
                        return Err(RepositoryError::Failed(
                            "operations alert 通知载荷无效".to_owned(),
                        )
                        .into());
                    }
                }
                Some(_) => {
                    if command.origin != MailDeliveryOrigin::MailIntake
                        || command.user_key_user_id.is_some()
                    {
                        return Err(RepositoryError::Failed(
                            "MoogleMail 普通通知不得携带用户 key 引用".to_owned(),
                        )
                        .into());
                    }
                }
                None => {
                    return Err(
                        RepositoryError::Failed("MoogleMail 通知模板缺失".to_owned()).into()
                    );
                }
            }
        }
        MailDeliveryOperation::Return => {
            if command.template.is_some()
                || command.result_code.is_some()
                || command.credits != 0
                || command.cod != 0
                || command.user_key_user_id.is_some()
                || command.origin == MailDeliveryOrigin::OperationsAlert
            {
                return Err(
                    RepositoryError::Failed("MoogleMail 退回/领取载荷越界".to_owned()).into()
                );
            }
        }
        MailDeliveryOperation::Take => {
            if command.template.is_some()
                || command.result_code.is_some()
                || command.user_key_user_id.is_some()
                || command.origin == MailDeliveryOrigin::OperationsAlert
            {
                return Err(RepositoryError::Failed("MoogleMail 领取载荷越界".to_owned()).into());
            }
            if command.origin == MailDeliveryOrigin::MailIntake
                && (command.credits != 0 || command.cod != 0)
            {
                return Err(
                    RepositoryError::Failed("MoogleMail 普通领取不得携带金额".to_owned()).into()
                );
            }
            if command.origin == MailDeliveryOrigin::SettlementRecoveryAction
                && command.attachment_ids.windows(2).any(|values| values[0] >= values[1])
            {
                return Err(RepositoryError::Failed(
                    "MoogleMail recovery 领取附件未按稳定顺序排列".to_owned(),
                )
                .into());
            }
        }
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn enqueue_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    command: &MailDeliveryCommand,
) -> Result<MailDeliveryResult, RepositoryError> {
    insert_postgres(pool, command, None).await
}

#[cfg(feature = "postgres")]
async fn prepare_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    command: &MailDeliveryCommand,
) -> Result<MailDeliveryResult, RepositoryError> {
    insert_postgres(pool, command, Some("prepared")).await
}

#[cfg(feature = "postgres")]
async fn insert_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
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

#[cfg(feature = "postgres")]
async fn activate_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    activation: &MailDeliveryActivation,
) -> Result<MailDeliveryResult, RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let row = sqlx::query(
        "SELECT id, command_id, request_digest, origin_kind, status,
                activation_command_id, activation_request_digest
         FROM public.deliveries WHERE id = $1 FOR UPDATE",
    )
    .bind(activation.delivery_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("recovery delivery 不存在"))?;
    let command_id: Uuid = sqlx::Row::try_get(&row, "command_id").map_err(map_storage_error)?;
    let request_digest: Vec<u8> =
        sqlx::Row::try_get(&row, "request_digest").map_err(map_storage_error)?;
    let origin_kind: String = sqlx::Row::try_get(&row, "origin_kind").map_err(map_storage_error)?;
    let status: String = sqlx::Row::try_get(&row, "status").map_err(map_storage_error)?;
    let existing_activate_command_id: Option<Uuid> =
        sqlx::Row::try_get(&row, "activation_command_id").map_err(map_storage_error)?;
    let existing_activate_digest: Option<Vec<u8>> =
        sqlx::Row::try_get(&row, "activation_request_digest").map_err(map_storage_error)?;
    if command_id != activation.prepare_command_id.as_uuid()
        || request_digest != activation.prepare_request_hash.as_bytes()
        || origin_kind != MailDeliveryOrigin::SettlementRecoveryAction.as_str()
    {
        return Err(repository_error("recovery delivery prepare identity 冲突"));
    }
    let activated = match status.as_str() {
        "prepared" => {
            if existing_activate_command_id.is_some() || existing_activate_digest.is_some() {
                return Err(repository_error("recovery delivery activation identity 已被占用"));
            }
            let updated = sqlx::query(
                "UPDATE public.deliveries
                 SET status = 'pending', activation_command_id = $2,
                     activation_request_digest = $3, activated_at = $4, updated_at = $4
                 WHERE id = $1 AND status = 'prepared'",
            )
            .bind(activation.delivery_id)
            .bind(activation.activate_command_id.as_uuid())
            .bind(activation.activate_request_hash.as_bytes().to_vec())
            .bind(activation.activated_at)
            .execute(&mut *transaction)
            .await
            .map_err(map_storage_error)?;
            if updated.rows_affected() != 1 {
                return Err(repository_error("recovery delivery 激活状态已变化"));
            }
            true
        }
        "pending" | "claimed" | "sending" | "succeeded" | "failed" | "manual_reconcile"
            if existing_activate_command_id == Some(activation.activate_command_id.as_uuid())
                && existing_activate_digest.as_deref()
                    == Some(activation.activate_request_hash.as_bytes().as_slice()) =>
        {
            false
        }
        _ => return Err(repository_error("recovery delivery 状态无法激活")),
    };
    if activated {
        let request_id = activation.activate_command_id.to_string();
        append_delivery_audit(
            &mut transaction,
            DeliveryAudit {
                occurred_at: activation.activated_at,
                actor_type: "system",
                actor_id: "delivery-service",
                action: ACTIVATE,
                delivery_id: activation.delivery_id,
                request_id: Some(&request_id),
                source: "system",
                metadata: json!({"status": "pending"}),
            },
        )
        .await
        .map_err(map_storage_error)?;
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(MailDeliveryResult { delivery_id: activation.delivery_id })
}

#[cfg(feature = "postgres")]
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

fn valid_notification_body(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        !value.is_empty()
            && value.len() <= (16 << 10)
            && value == value.trim()
            && !value
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    })
}

#[cfg(feature = "postgres")]
fn map_storage_error(error: sqlx::Error) -> RepositoryError {
    RepositoryError::Failed(match error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("40001") =>
        {
            "MoogleMail delivery 事务发生序列化冲突".to_owned()
        }
        _ => "MoogleMail delivery outbox 操作失败".to_owned(),
    })
}

#[cfg(feature = "postgres")]
fn repository_error(message: &str) -> RepositoryError {
    RepositoryError::Failed(message.to_owned())
}
