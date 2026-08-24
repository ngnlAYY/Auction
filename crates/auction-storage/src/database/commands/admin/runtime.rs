//! Admin runtime 配置的 PostgreSQL 命令账本适配器。
//!
//! runtime 文件和数据库账本是两个独立的持久化资源；本适配器只拥有账本、审计记录及其
//! pending/applied/unknown 状态，不直接决定文件替换顺序。SQLite Lite 不声明这项完整档能力。

use std::sync::Arc;

use auction_application::{
    AdminRuntimeAuditRecord, AdminRuntimeCommandStore, AdminRuntimeStoredCommand, ApplicationError,
    ApplicationFuture, RequestHash,
};
#[cfg(feature = "postgres")]
use auction_application::{
    AdminRuntimeCommandError, AdminRuntimeCommandState, AdminRuntimeDocument,
};
use auction_domain::CommandId;
use chrono::{DateTime, Utc};

#[cfg(feature = "postgres")]
use sqlx::{Row, postgres::PgRow};
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
use crate::database::audit_health::AuditWriteResult as _;

use super::DatabaseStore;

/// 绑定运行时数据库的 Admin runtime 命令账本端口。
pub struct DatabaseAdminRuntimeCommands {
    store: Arc<DatabaseStore>,
}

impl DatabaseAdminRuntimeCommands {
    /// 建立不向应用层暴露具体连接池类型的 runtime 账本端口。
    pub fn new(store: Arc<DatabaseStore>) -> Self {
        Self { store }
    }
}

impl AdminRuntimeCommandStore for DatabaseAdminRuntimeCommands {
    fn find(
        &self,
        command_id: CommandId,
    ) -> ApplicationFuture<'_, Option<AdminRuntimeStoredCommand>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => find_postgres(store, command_id).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    let _ = command_id;
                    Err(lite_unavailable())
                }
            }
        })
    }

    fn list_unresolved(&self) -> ApplicationFuture<'_, Vec<AdminRuntimeStoredCommand>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_unresolved_postgres(store).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(lite_unavailable()),
            }
        })
    }

    fn insert_pending(
        &self,
        command: AdminRuntimeStoredCommand,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => insert_pending_postgres(store, command).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    let _ = command;
                    Err(lite_unavailable())
                }
            }
        })
    }

    fn apply(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        audit: AdminRuntimeAuditRecord,
        applied_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    apply_postgres(store, command_id, request_hash, audit, applied_at).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    let _ = (command_id, request_hash, audit, applied_at);
                    Err(lite_unavailable())
                }
            }
        })
    }

    fn mark_unknown(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        category: String,
        updated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    mark_unknown_postgres(store, command_id, request_hash, category, updated_at)
                        .await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    let _ = (command_id, request_hash, category, updated_at);
                    Err(lite_unavailable())
                }
            }
        })
    }

    fn reopen(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        updated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    reopen_postgres(store, command_id, request_hash, updated_at).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    let _ = (command_id, request_hash, updated_at);
                    Err(lite_unavailable())
                }
            }
        })
    }
}

#[cfg(feature = "postgres")]
const SELECT_COMMAND_BY_ID: &str = "SELECT command_id, digest_version, request_digest, state,
       expected_fingerprint, old_fingerprint, new_fingerprint, candidate_document,
       restart_required, audit_id, actor_id, request_id, occurred_at, unknown_category,
       created_at, updated_at, applied_at
FROM public.runtime_config_admin_commands
WHERE command_id = $1";

#[cfg(feature = "postgres")]
const SELECT_UNRESOLVED: &str = "SELECT command_id, digest_version, request_digest, state,
       expected_fingerprint, old_fingerprint, new_fingerprint, candidate_document,
       restart_required, audit_id, actor_id, request_id, occurred_at, unknown_category,
       created_at, updated_at, applied_at
FROM public.runtime_config_admin_commands
WHERE state IN ('pending', 'unknown')
ORDER BY updated_at, command_id";

#[cfg(feature = "postgres")]
const SELECT_COMMAND_FOR_UPDATE: &str = "SELECT command_id, digest_version, request_digest, state,
       expected_fingerprint, old_fingerprint, new_fingerprint, candidate_document,
       restart_required, audit_id, actor_id, request_id, occurred_at, unknown_category,
       created_at, updated_at, applied_at
FROM public.runtime_config_admin_commands
WHERE command_id = $1
FOR UPDATE";

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

#[cfg(feature = "postgres")]
async fn find_postgres(
    store: &crate::PostgresStore,
    command_id: CommandId,
) -> Result<Option<AdminRuntimeStoredCommand>, ApplicationError> {
    let row = sqlx::query(SELECT_COMMAND_BY_ID)
        .bind(command_id.as_uuid())
        .fetch_optional(store.pool())
        .await
        .map_err(|_| command_failed("读取 runtime 配置命令失败"))?;
    row.map(map_command_row).transpose()
}

#[cfg(feature = "postgres")]
async fn list_unresolved_postgres(
    store: &crate::PostgresStore,
) -> Result<Vec<AdminRuntimeStoredCommand>, ApplicationError> {
    let rows = sqlx::query(SELECT_UNRESOLVED)
        .fetch_all(store.pool())
        .await
        .map_err(|_| command_failed("读取未完成的 runtime 配置命令失败"))?;
    rows.into_iter().map(map_command_row).collect()
}

#[cfg(feature = "postgres")]
async fn insert_pending_postgres(
    store: &crate::PostgresStore,
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
    .bind(command.request_hash.as_bytes().to_vec())
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
async fn apply_postgres(
    store: &crate::PostgresStore,
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
        .bind(request_hash.as_bytes().to_vec())
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
async fn mark_unknown_postgres(
    store: &crate::PostgresStore,
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
        .bind(request_hash.as_bytes().to_vec())
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
async fn reopen_postgres(
    store: &crate::PostgresStore,
    command_id: CommandId,
    request_hash: RequestHash,
    updated_at: DateTime<Utc>,
) -> Result<AdminRuntimeStoredCommand, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| command_failed("开启 runtime 配置重开事务失败"))?;
    let row = sqlx::query(UPDATE_REOPEN)
        .bind(command_id.as_uuid())
        .bind(request_hash.as_bytes().to_vec())
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
fn map_command_row(row: PgRow) -> Result<AdminRuntimeStoredCommand, ApplicationError> {
    let command_id: Uuid =
        row.try_get("command_id").map_err(|_| command_failed("runtime 命令标识读取失败"))?;
    let digest_version: i16 = row
        .try_get("digest_version")
        .map_err(|_| command_failed("runtime 命令摘要版本读取失败"))?;
    let request_digest: Vec<u8> =
        row.try_get("request_digest").map_err(|_| command_failed("runtime 命令摘要读取失败"))?;
    let request_digest: [u8; 32] =
        request_digest.try_into().map_err(|_| command_failed("runtime 命令摘要长度无效"))?;
    let candidate_bytes: Vec<u8> = row
        .try_get("candidate_document")
        .map_err(|_| command_failed("runtime 候选文档读取失败"))?;
    let candidate_document: AdminRuntimeDocument = serde_json::from_slice(&candidate_bytes)
        .map_err(|_| command_corrupt("runtime 候选文档不是有效 JSON".to_owned()))?;
    let state = match row
        .try_get::<String, _>("state")
        .map_err(|_| command_failed("runtime 命令状态读取失败"))?
        .as_str()
    {
        "pending" => AdminRuntimeCommandState::Pending,
        "applied" => AdminRuntimeCommandState::Applied,
        "unknown" => AdminRuntimeCommandState::Unknown,
        _ => return Err(command_corrupt("runtime 命令状态无效".to_owned())),
    };
    let unknown_category: String = row
        .try_get("unknown_category")
        .map_err(|_| command_failed("runtime 命令未知分类读取失败"))?;
    let result_document =
        (state == AdminRuntimeCommandState::Applied).then(|| candidate_document.clone());
    let command = AdminRuntimeStoredCommand {
        command_id: CommandId::from_uuid(command_id),
        audit_id: row
            .try_get("audit_id")
            .map_err(|_| command_failed("runtime 审计标识读取失败"))?,
        digest_version: u16::try_from(digest_version)
            .map_err(|_| command_corrupt("runtime 命令摘要版本无效".to_owned()))?,
        request_hash: RequestHash::new(request_digest),
        expected_fingerprint: row
            .try_get("expected_fingerprint")
            .map_err(|_| command_failed("runtime expected fingerprint 读取失败"))?,
        old_fingerprint: row
            .try_get("old_fingerprint")
            .map_err(|_| command_failed("runtime old fingerprint 读取失败"))?,
        new_fingerprint: row
            .try_get("new_fingerprint")
            .map_err(|_| command_failed("runtime new fingerprint 读取失败"))?,
        candidate_document,
        result_document,
        restart_required: row
            .try_get("restart_required")
            .map_err(|_| command_failed("runtime restart 标记读取失败"))?,
        actor_id: row.try_get("actor_id").map_err(|_| command_failed("runtime actor 读取失败"))?,
        request_id: row
            .try_get("request_id")
            .map_err(|_| command_failed("runtime request_id 读取失败"))?,
        occurred_at: row
            .try_get("occurred_at")
            .map_err(|_| command_failed("runtime occurred_at 读取失败"))?,
        state,
        unknown_category: (!unknown_category.is_empty()).then_some(unknown_category),
        created_at: row
            .try_get("created_at")
            .map_err(|_| command_failed("runtime created_at 读取失败"))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| command_failed("runtime updated_at 读取失败"))?,
        applied_at: row
            .try_get("applied_at")
            .map_err(|_| command_failed("runtime applied_at 读取失败"))?,
    };
    command.validate().map_err(|error| command_corrupt(error.to_string()))?;
    Ok(command)
}

#[cfg(feature = "postgres")]
fn command_failed(message: &str) -> ApplicationError {
    AdminRuntimeCommandError::Failed(message.to_owned()).into()
}

#[cfg(feature = "postgres")]
fn command_corrupt(message: String) -> ApplicationError {
    AdminRuntimeCommandError::Failed(format!("runtime 命令账本损坏：{message}")).into()
}

#[cfg(feature = "postgres")]
fn map_command_sql_error(error: sqlx::Error) -> ApplicationError {
    match error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("23505") =>
        {
            AdminRuntimeCommandError::Conflict.into()
        }
        _ => command_failed("PostgreSQL runtime 配置命令操作失败"),
    }
}

#[cfg(feature = "sqlite")]
fn lite_unavailable() -> ApplicationError {
    ApplicationError::CapabilityUnavailable("Admin runtime 配置更新仅支持 PostgreSQL 完整档")
}
