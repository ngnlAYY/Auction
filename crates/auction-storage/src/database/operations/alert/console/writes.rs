//! 告警确认与静默：只追加操作者事实（静默时推进 next_notify_at），命令可按原 ID 重放。

use auction_application::core::ApplicationError;
use auction_application::operations::{
    OperationsAlertAcknowledgeCommand, OperationsAlertCommandResult, OperationsAlertError,
    OperationsAlertSilenceCommand,
};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::database::{lock_command_id, storage_error_from};

/// 操作者对告警的动作；静默携带已校验的时长。
#[derive(Clone, Copy)]
enum OperatorAction {
    Acknowledge,
    Silence { seconds: i64 },
}

impl OperatorAction {
    /// 事实表与重放比对使用的动作名。
    fn name(self) -> &'static str {
        match self {
            Self::Acknowledge => "acknowledge",
            Self::Silence { .. } => "silence",
        }
    }

    /// 诊断上下文按动作区分，日志能看出失败的是确认还是静默。
    fn context(self, acknowledge: &'static str, silence: &'static str) -> &'static str {
        match self {
            Self::Acknowledge => acknowledge,
            Self::Silence { .. } => silence,
        }
    }
}

/// 确认与静默共用的命令身份与输入；端口已完成校验。
pub(super) struct OperatorCommand<'a> {
    command_id: Uuid,
    fingerprint: [u8; 32],
    request_digest: [u8; 32],
    actor_id: &'a str,
    reason: &'a str,
    action: OperatorAction,
}

impl<'a> OperatorCommand<'a> {
    pub(super) fn acknowledge(command: &'a OperationsAlertAcknowledgeCommand) -> Self {
        Self {
            command_id: command.command_id,
            fingerprint: command.fingerprint,
            request_digest: command.request_hash.as_bytes(),
            actor_id: &command.actor_id,
            reason: &command.reason,
            action: OperatorAction::Acknowledge,
        }
    }

    pub(super) fn silence(command: &'a OperationsAlertSilenceCommand) -> Self {
        Self {
            command_id: command.command_id,
            fingerprint: command.fingerprint,
            request_digest: command.request_hash.as_bytes(),
            actor_id: &command.actor_id,
            reason: &command.reason,
            action: OperatorAction::Silence { seconds: command.silence_seconds },
        }
    }
}

/// 同一事务内：锁命令 ID → 同 ID 重放（不变量 1）→ 锁 signal 并要求 active → 只追加操作者事实
/// （不变量 8）→ 更新投影。确认与静默都不改写 kind/scope/source/count/age/active。
pub(super) async fn write_operator_fact_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    command: OperatorCommand<'_>,
) -> Result<OperationsAlertCommandResult, ApplicationError> {
    let action = command.action;
    let mut transaction = pool.begin().await.map_err(|error| {
        storage_error_from(error, action.context("开启告警确认事务失败", "开启告警静默事务失败"))
    })?;
    lock_command_id(&mut transaction, command.command_id).await.map_err(|error| {
        storage_error_from(error, action.context("锁定告警确认命令失败", "锁定告警静默命令失败"))
    })?;
    if let Some(result) = replay_postgres(&mut transaction, &command).await? {
        transaction.commit().await.map_err(|error| {
            storage_error_from(
                error,
                action.context("提交告警确认重放事务失败", "提交告警静默重放事务失败"),
            )
        })?;
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
    let silenced_until = match action {
        OperatorAction::Acknowledge => None,
        OperatorAction::Silence { seconds } => Some(
            occurred_at
                .checked_add_signed(chrono::Duration::seconds(seconds))
                .ok_or(ApplicationError::OperationsAlert(OperationsAlertError::Invalid))?,
        ),
    };
    insert_fact(&mut transaction, &command, silenced_until, occurred_at).await?;
    let projection = match silenced_until {
        None => sqlx::query(
            "UPDATE public.operations_alert_signals
             SET acknowledged_at = $2, acknowledged_by = $3
             WHERE fingerprint = $1",
        )
        .bind(command.fingerprint.as_slice())
        .bind(occurred_at)
        .bind(command.actor_id),
        // 静默只推进重复通知时间；active/kind/scope/count/age/state_version 保持不变，
        // 到期后下一次扫描仍按原阈值继续评估。
        Some(silenced_until) => sqlx::query(
            "UPDATE public.operations_alert_signals
             SET silenced_until = GREATEST(COALESCE(silenced_until, $2), $2),
                 next_notify_at = GREATEST(next_notify_at, $2)
             WHERE fingerprint = $1 AND active",
        )
        .bind(command.fingerprint.as_slice())
        .bind(silenced_until),
    };
    projection.execute(&mut *transaction).await.map_err(|error| {
        storage_error_from(error, action.context("写入告警确认投影失败", "写入告警静默投影失败"))
    })?;
    transaction.commit().await.map_err(|error| {
        storage_error_from(error, action.context("提交告警确认事务失败", "提交告警静默事务失败"))
    })?;
    Ok(OperationsAlertCommandResult {
        command_id: command.command_id,
        fingerprint: command.fingerprint,
        action: action.name().to_owned(),
        silenced_until,
        replayed: false,
    })
}

/// 同命令 ID 重放：摘要、目标与动作一致返回原结果，否则失败关闭。
async fn replay_postgres(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command: &OperatorCommand<'_>,
) -> Result<Option<OperationsAlertCommandResult>, ApplicationError> {
    let row = sqlx::query(
        "SELECT fingerprint, action, request_digest, silenced_until
         FROM public.operations_alert_acknowledgements
         WHERE command_id = $1",
    )
    .bind(command.command_id)
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
    if stored_fingerprint.as_slice() != command.fingerprint
        || stored_digest.as_slice() != command.request_digest
        || stored_action != command.action.name()
    {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::IdentityConflict));
    }
    Ok(Some(OperationsAlertCommandResult {
        command_id: command.command_id,
        fingerprint: command.fingerprint,
        action: stored_action,
        silenced_until,
        replayed: true,
    }))
}

/// 追加一行操作者事实；时长与静默截止只在静默时有值。
async fn insert_fact(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command: &OperatorCommand<'_>,
    silenced_until: Option<DateTime<Utc>>,
    occurred_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let silence_seconds = match command.action {
        OperatorAction::Acknowledge => None,
        OperatorAction::Silence { seconds } => Some(seconds),
    };
    sqlx::query(
        "INSERT INTO public.operations_alert_acknowledgements
            (fingerprint, action, command_id, request_digest, actor_id, reason,
             silence_seconds, silenced_until, occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(command.fingerprint.as_slice())
    .bind(command.action.name())
    .bind(command.command_id)
    .bind(command.request_digest.as_slice())
    .bind(command.actor_id)
    .bind(command.reason)
    .bind(silence_seconds)
    .bind(silenced_until)
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入告警确认事实失败"))?;
    Ok(())
}
