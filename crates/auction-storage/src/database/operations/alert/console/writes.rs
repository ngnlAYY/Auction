//! 告警确认与静默：只追加操作者事实（静默时推进 next_notify_at），命令可按原 ID 重放。

use auction_application::core::ApplicationError;
use auction_application::operations::{
    OperationsAlertAcknowledgeCommand, OperationsAlertCommandResult, OperationsAlertError,
    OperationsAlertSilenceCommand,
};
use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::database::{lock_command_id, storage_error_from};

pub(super) async fn write_acknowledgement_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    command: &OperationsAlertAcknowledgeCommand,
    silenced_until: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<OperationsAlertCommandResult, ApplicationError> {
    let action = if silenced_until.is_some() { "silence" } else { "acknowledge" };
    let mut transaction =
        pool.begin().await.map_err(|error| storage_error_from(error, "开启告警确认事务失败"))?;
    lock_command_id(&mut transaction, command.command_id)
        .await
        .map_err(|error| storage_error_from(error, "锁定告警确认命令失败"))?;
    if let Some(result) = replay_postgres(
        &mut transaction,
        command.command_id,
        command.fingerprint,
        command.request_hash.as_bytes(),
        action,
    )
    .await?
    {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交告警确认重放事务失败"))?;
        return Ok(result);
    }
    let signal = sqlx::query(
        "SELECT active FROM public.operations_alert_signals
         WHERE fingerprint = $1
         FOR UPDATE",
    )
    .bind(command.fingerprint.as_slice())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取告警水位失败"))?
    .ok_or(ApplicationError::OperationsAlert(OperationsAlertError::Invalid))?;
    let active: bool = signal
        .try_get("active")
        .map_err(|error| storage_error_from(error, "读取告警活动状态失败"))?;
    if !active {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
    }
    let occurred_at = Utc::now();
    insert_fact(
        &mut transaction,
        command.command_id,
        command.fingerprint,
        action,
        command.request_hash.as_bytes(),
        &command.actor_id,
        &command.reason,
        None,
        None,
        occurred_at,
    )
    .await?;
    sqlx::query(
        "UPDATE public.operations_alert_signals
         SET acknowledged_at = $2, acknowledged_by = $3
         WHERE fingerprint = $1",
    )
    .bind(command.fingerprint.as_slice())
    .bind(occurred_at)
    .bind(&command.actor_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入告警确认投影失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交告警确认事务失败"))?;
    Ok(OperationsAlertCommandResult {
        command_id: command.command_id,
        fingerprint: command.fingerprint,
        action: action.to_owned(),
        silenced_until: None,
        replayed: false,
    })
}

pub(super) async fn write_silence_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    command: &OperationsAlertSilenceCommand,
) -> Result<OperationsAlertCommandResult, ApplicationError> {
    let mut transaction =
        pool.begin().await.map_err(|error| storage_error_from(error, "开启告警静默事务失败"))?;
    lock_command_id(&mut transaction, command.command_id)
        .await
        .map_err(|error| storage_error_from(error, "锁定告警静默命令失败"))?;
    let action = "silence";
    if let Some(result) = replay_postgres(
        &mut transaction,
        command.command_id,
        command.fingerprint,
        command.request_hash.as_bytes(),
        action,
    )
    .await?
    {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交告警静默重放事务失败"))?;
        return Ok(result);
    }
    let signal = sqlx::query(
        "SELECT active FROM public.operations_alert_signals
         WHERE fingerprint = $1
         FOR UPDATE",
    )
    .bind(command.fingerprint.as_slice())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取告警水位失败"))?
    .ok_or(ApplicationError::OperationsAlert(OperationsAlertError::Invalid))?;
    let active: bool = signal
        .try_get("active")
        .map_err(|error| storage_error_from(error, "读取告警活动状态失败"))?;
    if !active {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
    }
    let occurred_at = Utc::now();
    let silenced_until = occurred_at
        .checked_add_signed(chrono::Duration::seconds(command.silence_seconds))
        .ok_or(ApplicationError::OperationsAlert(OperationsAlertError::Invalid))?;
    insert_fact(
        &mut transaction,
        command.command_id,
        command.fingerprint,
        action,
        command.request_hash.as_bytes(),
        &command.actor_id,
        &command.reason,
        Some(command.silence_seconds),
        Some(silenced_until),
        occurred_at,
    )
    .await?;
    // 只推进重复通知时间；active/kind/scope/count/age/state_version 保持不变，
    // 到期后下一次扫描仍按原阈值继续评估。
    sqlx::query(
        "UPDATE public.operations_alert_signals
         SET silenced_until = GREATEST(COALESCE(silenced_until, $2), $2),
             next_notify_at = GREATEST(next_notify_at, $2)
         WHERE fingerprint = $1 AND active",
    )
    .bind(command.fingerprint.as_slice())
    .bind(silenced_until)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入告警静默投影失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交告警静默事务失败"))?;
    Ok(OperationsAlertCommandResult {
        command_id: command.command_id,
        fingerprint: command.fingerprint,
        action: action.to_owned(),
        silenced_until: Some(silenced_until),
        replayed: false,
    })
}

/// 同命令 ID 重放：摘要与目标一致返回原结果，否则失败关闭。
pub(super) async fn replay_postgres(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command_id: uuid::Uuid,
    fingerprint: [u8; 32],
    request_digest: [u8; 32],
    action: &str,
) -> Result<Option<OperationsAlertCommandResult>, ApplicationError> {
    let row = sqlx::query(
        "SELECT fingerprint, action, request_digest, silenced_until
         FROM public.operations_alert_acknowledgements
         WHERE command_id = $1",
    )
    .bind(command_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取告警确认重放失败"))?;
    let Some(row) = row else { return Ok(None) };
    let stored_fingerprint: Vec<u8> = row
        .try_get("fingerprint")
        .map_err(|error| storage_error_from(error, "读取告警重放标识失败"))?;
    let stored_digest: Vec<u8> = row
        .try_get("request_digest")
        .map_err(|error| storage_error_from(error, "读取告警重放摘要失败"))?;
    let stored_action: String =
        row.try_get("action").map_err(|error| storage_error_from(error, "读取告警重放动作失败"))?;
    let silenced_until: Option<DateTime<Utc>> = row
        .try_get("silenced_until")
        .map_err(|error| storage_error_from(error, "读取告警重放静默时间失败"))?;
    if stored_fingerprint.as_slice() != fingerprint
        || stored_digest.as_slice() != request_digest
        || stored_action != action
    {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::IdentityConflict));
    }
    Ok(Some(OperationsAlertCommandResult {
        command_id,
        fingerprint,
        action: stored_action,
        silenced_until,
        replayed: true,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn insert_fact(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command_id: uuid::Uuid,
    fingerprint: [u8; 32],
    action: &str,
    request_digest: [u8; 32],
    actor_id: &str,
    reason: &str,
    silence_seconds: Option<i64>,
    silenced_until: Option<DateTime<Utc>>,
    occurred_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.operations_alert_acknowledgements
            (fingerprint, action, command_id, request_digest, actor_id, reason,
             silence_seconds, silenced_until, occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(fingerprint.as_slice())
    .bind(action)
    .bind(command_id)
    .bind(request_digest.as_slice())
    .bind(actor_id)
    .bind(reason)
    .bind(silence_seconds)
    .bind(silenced_until)
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入告警确认事实失败"))?;
    Ok(())
}
