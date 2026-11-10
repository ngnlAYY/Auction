//! PostgreSQL runtime 命令账本的 pending、applied、unknown 和 reopen 事务。

#[cfg(feature = "postgres")]
use auction_application::admin::{
    AdminRuntimeAuditRecord, AdminRuntimeCommandError, AdminRuntimeCommandState,
    AdminRuntimeStoredCommand,
};
#[cfg(feature = "postgres")]
use auction_application::core::{ApplicationError, RequestHash};
#[cfg(feature = "postgres")]
use auction_domain::CommandId;
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};

#[cfg(feature = "postgres")]
use crate::database::audit_health::AuditWriteResult as _;

#[cfg(feature = "postgres")]
use super::{
    errors::{command_corrupt, command_failed, map_command_sql_error},
    mapping::map_command_row,
    queries::SELECT_COMMAND_FOR_UPDATE,
};

#[cfg(feature = "postgres")]
pub(super) async fn insert_pending_postgres(
    store: &crate::DatabaseStore,
    command: AdminRuntimeStoredCommand,
) -> Result<AdminRuntimeStoredCommand, ApplicationError> {
    command.validate().map_err(|error| command_corrupt(error.to_string()))?;
    let candidate_document = serde_json::to_vec(&command.candidate_document)
        .map_err(|_| command_failed("编码 runtime 配置候选文档失败"))?;
    let mut transaction =
        store.pool().begin().await.map_err(|_| command_failed("开启 runtime 配置命令事务失败"))?;

    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, $2, 'admin', $3, 'config.runtime_update', 'runtime_config', 'current',
                 'success', $4, 'admin', $5)",
    )
    .bind(command.audit_id)
    .bind(command.occurred_at)
    .bind(&command.actor_id)
    .bind(&command.request_id)
    .bind(sqlx::types::Json(serde_json::json!({
        "command_id": command.command_id.as_uuid(),
        "expected_fingerprint": command.expected_fingerprint,
        "old_fingerprint": command.old_fingerprint,
        "new_fingerprint": command.new_fingerprint,
        "state": "pending",
    })))
    .execute(&mut *transaction)
    .await
    .record_audit_failure()
    .map_err(map_command_sql_error)?;

    sqlx::query(
        "INSERT INTO public.runtime_config_admin_commands
            (command_id, digest_version, request_digest, state, expected_fingerprint,
             old_fingerprint, new_fingerprint, candidate_document, restart_required, audit_id,
             actor_id, request_id, occurred_at, unknown_category, created_at, updated_at, applied_at)
         VALUES ($1, $2, $3, 'pending', $4, $5, $6, $7, TRUE, $8, $9, $10, $11, '', $12, $13, NULL)",
    )
    .bind(command.command_id.as_uuid())
    .bind(i16::try_from(command.digest_version).map_err(|_| command_corrupt("digest version 超出 PostgreSQL smallint".to_owned()))?)
    .bind(command.request_hash.as_bytes().as_slice())
    .bind(&command.expected_fingerprint)
    .bind(&command.old_fingerprint)
    .bind(&command.new_fingerprint)
    .bind(candidate_document)
    .bind(command.audit_id)
    .bind(&command.actor_id)
    .bind(&command.request_id)
    .bind(command.occurred_at)
    .bind(command.created_at)
    .bind(command.updated_at)
    .execute(&mut *transaction)
    .await
    .map_err(map_command_sql_error)?;

    transaction.commit().await.map_err(|_| command_failed("提交 runtime 配置命令事务失败"))?;
    Ok(command)
}

#[cfg(feature = "postgres")]
pub(super) async fn apply_postgres(
    store: &crate::DatabaseStore,
    command_id: CommandId,
    request_hash: RequestHash,
    audit: AdminRuntimeAuditRecord,
    applied_at: DateTime<Utc>,
) -> Result<AdminRuntimeStoredCommand, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| command_failed("开启 runtime 配置应用事务失败"))?;
    let current = sqlx::query(SELECT_COMMAND_FOR_UPDATE)
        .bind(command_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| command_failed("锁定 runtime 配置命令失败"))?
        .map(map_command_row)
        .transpose()?;
    let Some(current) = current else {
        return Err(AdminRuntimeCommandError::NotFound.into());
    };
    if !current.matches_request(request_hash)
        || current.audit_id != audit.id
        || current.old_fingerprint != audit.old_fingerprint
        || current.new_fingerprint != audit.new_fingerprint
    {
        return Err(AdminRuntimeCommandError::Conflict.into());
    }
    if current.state == AdminRuntimeCommandState::Applied {
        transaction.commit().await.map_err(|_| command_failed("提交 runtime 配置重放事务失败"))?;
        return Ok(current);
    }

    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, $2, 'admin', $3, 'config.runtime_update', 'runtime_config', 'current',
                 'success', $4, 'admin', $5)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(audit.id)
    .bind(audit.occurred_at)
    .bind(&audit.actor_id)
    .bind(&audit.request_id)
    .bind(sqlx::types::Json(serde_json::json!({
        "command_id": audit.command_id.as_uuid(),
        "old_fingerprint": audit.old_fingerprint,
        "new_fingerprint": audit.new_fingerprint,
        "state": "applied",
        "applied_at": applied_at,
    })))
    .execute(&mut *transaction)
    .await
    .record_audit_failure()
    .map_err(map_command_sql_error)?;

    let row = sqlx::query(UPDATE_APPLY)
        .bind(command_id.as_uuid())
        .bind(request_hash.as_bytes().as_slice())
        .bind(applied_at)
        .bind(applied_at)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| command_failed("应用 runtime 配置命令失败"))?;
    let applied = row
        .map(map_command_row)
        .transpose()?
        .ok_or_else(|| command_failed("runtime 配置命令状态在应用过程中发生变化"))?;
    transaction.commit().await.map_err(|_| command_failed("提交 runtime 配置应用事务失败"))?;
    Ok(applied)
}

#[cfg(feature = "postgres")]
pub(super) async fn mark_unknown_postgres(
    store: &crate::DatabaseStore,
    command_id: CommandId,
    request_hash: RequestHash,
    category: String,
    updated_at: DateTime<Utc>,
) -> Result<AdminRuntimeStoredCommand, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| command_failed("开启 runtime 配置未知状态事务失败"))?;
    let row = sqlx::query(UPDATE_UNKNOWN)
        .bind(command_id.as_uuid())
        .bind(request_hash.as_bytes().as_slice())
        .bind(category)
        .bind(updated_at)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_command_sql_error)?;
    let Some(row) = row else {
        let current = sqlx::query(SELECT_COMMAND_FOR_UPDATE)
            .bind(command_id.as_uuid())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| command_failed("读取 runtime 配置未知命令失败"))?
            .map(map_command_row)
            .transpose()?;
        let Some(current) = current else {
            return Err(AdminRuntimeCommandError::NotFound.into());
        };
        if !current.matches_request(request_hash) {
            return Err(AdminRuntimeCommandError::Conflict.into());
        }
        if current.state == AdminRuntimeCommandState::Unknown {
            transaction
                .commit()
                .await
                .map_err(|_| command_failed("提交 runtime 配置未知状态重放事务失败"))?;
            return Ok(current);
        }
        return Err(AdminRuntimeCommandError::Conflict.into());
    };
    let unknown = map_command_row(row)?;
    transaction.commit().await.map_err(|_| command_failed("提交 runtime 配置未知状态事务失败"))?;
    Ok(unknown)
}

#[cfg(feature = "postgres")]
pub(super) async fn reopen_postgres(
    store: &crate::DatabaseStore,
    command_id: CommandId,
    request_hash: RequestHash,
    updated_at: DateTime<Utc>,
) -> Result<AdminRuntimeStoredCommand, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| command_failed("开启 runtime 配置重开事务失败"))?;
    let row = sqlx::query(UPDATE_REOPEN)
        .bind(command_id.as_uuid())
        .bind(request_hash.as_bytes().as_slice())
        .bind(updated_at)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_command_sql_error)?;
    let Some(row) = row else {
        let current = sqlx::query(SELECT_COMMAND_FOR_UPDATE)
            .bind(command_id.as_uuid())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| command_failed("读取 runtime 配置重开命令失败"))?
            .map(map_command_row)
            .transpose()?;
        let Some(current) = current else {
            return Err(AdminRuntimeCommandError::NotFound.into());
        };
        if !current.matches_request(request_hash) {
            return Err(AdminRuntimeCommandError::Conflict.into());
        }
        if current.state == AdminRuntimeCommandState::Pending {
            transaction
                .commit()
                .await
                .map_err(|_| command_failed("提交 runtime 配置 pending 重放事务失败"))?;
            return Ok(current);
        }
        return Err(AdminRuntimeCommandError::Conflict.into());
    };
    let pending = map_command_row(row)?;
    transaction.commit().await.map_err(|_| command_failed("提交 runtime 配置重开事务失败"))?;
    Ok(pending)
}

#[cfg(feature = "postgres")]
const UPDATE_APPLY: &str = "UPDATE public.runtime_config_admin_commands
SET state = 'applied', unknown_category = '', updated_at = $3, applied_at = $4
WHERE command_id = $1 AND request_digest = $2
  AND state IN ('pending', 'unknown')
RETURNING command_id, digest_version, request_digest, state,
          expected_fingerprint, old_fingerprint, new_fingerprint, candidate_document,
          restart_required, audit_id, actor_id, request_id, occurred_at,
          unknown_category, created_at, updated_at, applied_at";

#[cfg(feature = "postgres")]
const UPDATE_UNKNOWN: &str = "UPDATE public.runtime_config_admin_commands
SET state = 'unknown', unknown_category = $3, updated_at = $4, applied_at = NULL
WHERE command_id = $1 AND request_digest = $2
  AND state IN ('pending', 'unknown')
RETURNING command_id, digest_version, request_digest, state,
          expected_fingerprint, old_fingerprint, new_fingerprint, candidate_document,
          restart_required, audit_id, actor_id, request_id, occurred_at,
          unknown_category, created_at, updated_at, applied_at";

#[cfg(feature = "postgres")]
const UPDATE_REOPEN: &str = "UPDATE public.runtime_config_admin_commands
SET state = 'pending', unknown_category = '', updated_at = $3
WHERE command_id = $1 AND request_digest = $2 AND state = 'unknown'
RETURNING command_id, digest_version, request_digest, state,
          expected_fingerprint, old_fingerprint, new_fingerprint, candidate_document,
          restart_required, audit_id, actor_id, request_id, occurred_at,
          unknown_category, created_at, updated_at, applied_at";
