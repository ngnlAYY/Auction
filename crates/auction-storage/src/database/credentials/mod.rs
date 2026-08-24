//! Admin 凭据 storage owner。
//!
//! 凭据只在 PostgreSQL 完整档落库。每个写命令使用 Serializable 事务和命令 advisory lock，
//! 将凭据行、命令账本、审计记录原子提交；SQLite Lite 显式报告能力不可用，不创建第二套
//! 不完整的凭据语义。

#[cfg(feature = "postgres")]
use auction_application::AdminCredentialEnvelope;
#[cfg(feature = "postgres")]
use auction_application::AdminCredentialHealthScope;
#[cfg(feature = "postgres")]
use auction_application::{
    AdminCredential, AdminCredentialDeleteResult, AdminCredentialStatus, RepositoryError,
};
use auction_application::{
    AdminCredentialCommandKind, AdminCredentialCommandResult, AdminCredentialDeleteWrite,
    AdminCredentialError, AdminCredentialHealthReader, AdminCredentialHealthSnapshot,
    AdminCredentialImportWrite, AdminCredentialKind, AdminCredentialListRequest,
    AdminCredentialMutationWrite, AdminCredentialPage, AdminCredentialRuntimeReader,
    AdminCredentialRuntimeRecord, AdminCredentialRuntimeWriter, AdminCredentialStore,
    AdminCredentialWorld, ApplicationError, ApplicationFuture, RequestHash,
};
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "postgres")]
use serde_json::{Value, json};
use uuid::Uuid;

#[cfg(feature = "postgres")]
use crate::database::audit_health::AuditWriteResult as _;

#[cfg(feature = "postgres")]
use sqlx::{Row, postgres::PgRow, types::Json};

use super::DatabaseStore;

impl AdminCredentialStore for DatabaseStore {
    fn list(
        &self,
        request: AdminCredentialListRequest,
    ) -> ApplicationFuture<'_, AdminCredentialPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(admin_error(AdminCredentialError::InvalidRequest));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }

    fn find_command(
        &self,
        command_id: Uuid,
        kind: AdminCredentialCommandKind,
        request_hash: RequestHash,
    ) -> ApplicationFuture<'_, Option<AdminCredentialCommandResult>> {
        Box::pin(async move {
            #[cfg(feature = "sqlite")]
            let _ = &kind;
            if command_id.is_nil() || request_hash.as_bytes() == [0; 32] {
                return Err(admin_error(AdminCredentialError::InvalidRequest));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    find_command_postgres(store, command_id, kind, request_hash).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }

    fn import(
        &self,
        write: AdminCredentialImportWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move {
            #[cfg(feature = "sqlite")]
            let _ = &write;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => import_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }

    fn mutate(
        &self,
        write: AdminCredentialMutationWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move {
            #[cfg(feature = "sqlite")]
            let _ = &write;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => mutate_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }

    fn delete(
        &self,
        write: AdminCredentialDeleteWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move {
            #[cfg(feature = "sqlite")]
            let _ = &write;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => delete_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }
}

impl AdminCredentialRuntimeReader for DatabaseStore {
    fn list_active(
        &self,
        kind: AdminCredentialKind,
        world: AdminCredentialWorld,
    ) -> ApplicationFuture<'_, Vec<AdminCredentialRuntimeRecord>> {
        Box::pin(async move {
            #[cfg(not(feature = "postgres"))]
            let _ = (kind, world);
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => list_active_postgres(store, kind, world).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }
}

impl AdminCredentialRuntimeWriter for DatabaseStore {
    fn mark_invalid(
        &self,
        credential_id: Uuid,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if credential_id.is_nil() || now.timestamp_nanos_opt().is_none() {
                return Err(admin_error(AdminCredentialError::InvalidRequest));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => mark_invalid_postgres(store, credential_id, now).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }
}

impl AdminCredentialHealthReader for DatabaseStore {
    fn health(&self) -> ApplicationFuture<'_, AdminCredentialHealthSnapshot> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => health_postgres(store).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCredentialPayload {
    credential: Option<AdminCredential>,
    deleted: Option<AdminCredentialDeleteResult>,
}

fn admin_error(error: AdminCredentialError) -> ApplicationError {
    ApplicationError::AdminCredential(error)
}

#[cfg(feature = "sqlite")]
fn capability_unavailable() -> ApplicationError {
    ApplicationError::CapabilityUnavailable("Admin 凭据管理只支持 PostgreSQL 完整档")
}

#[cfg(feature = "postgres")]
fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}

#[cfg(feature = "postgres")]
async fn list_active_postgres(
    store: &super::super::PostgresStore,
    kind: AdminCredentialKind,
    world: AdminCredentialWorld,
) -> Result<Vec<AdminCredentialRuntimeRecord>, ApplicationError> {
    let scope = world.scope(kind);
    let shared_scope = AdminCredentialWorld::Shared.scope(kind);
    let rows = sqlx::query(
        "SELECT id, scope, ciphertext, nonce, encryption_key_id, fingerprint, created_at
         FROM public.credential_keys
         WHERE credential_kind = $1
           AND status = 'active'
           AND scope IN ($2, $3)
         ORDER BY created_at ASC, id ASC",
    )
    .bind(kind.as_str())
    .bind(scope)
    .bind(shared_scope)
    .fetch_all(store.pool())
    .await
    .map_err(map_storage_error)?;
    rows.into_iter()
        .map(|row| {
            let envelope = AdminCredentialEnvelope::new(
                row.try_get("ciphertext").map_err(map_storage_error)?,
                row.try_get("nonce").map_err(map_storage_error)?,
                row.try_get("encryption_key_id").map_err(map_storage_error)?,
                row.try_get("fingerprint").map_err(map_storage_error)?,
            )
            .map_err(admin_error)?;
            let record = AdminCredentialRuntimeRecord {
                id: row.try_get("id").map_err(map_storage_error)?,
                kind,
                scope: row.try_get("scope").map_err(map_storage_error)?,
                envelope,
                created_at: row.try_get("created_at").map_err(map_storage_error)?,
            };
            record
                .validate()
                .then_some(record)
                .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))
        })
        .collect()
}

#[cfg(feature = "postgres")]
async fn mark_invalid_postgres(
    store: &super::super::PostgresStore,
    credential_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), ApplicationError> {
    let affected = sqlx::query(
        "UPDATE public.credential_keys
         SET status = 'invalid', state_version = state_version + 1, updated_at = $2
         WHERE id = $1 AND status <> 'invalid'",
    )
    .bind(credential_id)
    .bind(now)
    .execute(store.pool())
    .await
    .map_err(map_storage_error)?
    .rows_affected();
    if affected == 1 {
        return Ok(());
    }
    let status = sqlx::query_scalar::<_, Option<String>>(
        "SELECT status FROM public.credential_keys WHERE id = $1",
    )
    .bind(credential_id)
    .fetch_optional(store.pool())
    .await
    .map_err(map_storage_error)?
    .flatten();
    match status.as_deref() {
        Some("invalid") => Ok(()),
        Some(_) => Err(storage_error("确认凭据失效状态失败")),
        None => Err(admin_error(AdminCredentialError::NotFound)),
    }
}

#[cfg(feature = "postgres")]
async fn health_postgres(
    store: &super::super::PostgresStore,
) -> Result<AdminCredentialHealthSnapshot, ApplicationError> {
    let last_admin_change_at = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT MAX(created_at) FROM public.credential_admin_commands",
    )
    .fetch_one(store.pool())
    .await
    .map_err(map_storage_error)?;
    let rows = sqlx::query(
        "SELECT credential_kind, scope, status, count(*)::bigint AS count,
                MAX(updated_at)::timestamptz AS updated_at
         FROM public.credential_keys
         WHERE credential_kind IN ('mail_cookie', 'forum_cookie')
         GROUP BY credential_kind, scope, status
         ORDER BY credential_kind, scope, status",
    )
    .fetch_all(store.pool())
    .await
    .map_err(map_storage_error)?;
    let mut scopes = Vec::new();
    for row in rows {
        let kind = AdminCredentialKind::parse(
            &row.try_get::<String, _>("credential_kind").map_err(map_storage_error)?,
        )
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
        let scope = row.try_get::<String, _>("scope").map_err(map_storage_error)?;
        let world = world_from_scope(kind, &scope)
            .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
        let status = AdminCredentialStatus::parse(
            &row.try_get::<String, _>("status").map_err(map_storage_error)?,
        )
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
        let count = row.try_get::<i64, _>("count").map_err(map_storage_error)?;
        let updated_at =
            row.try_get::<Option<DateTime<Utc>>, _>("updated_at").map_err(map_storage_error)?;
        if count < 0 {
            return Err(admin_error(AdminCredentialError::StoredResultCorrupt));
        }
        let Some(scope_row) = scopes.iter_mut().find(|item: &&mut AdminCredentialHealthScope| {
            item.kind == kind && item.world == world
        }) else {
            scopes.push(AdminCredentialHealthScope {
                kind,
                world,
                active_count: 0,
                disabled_count: 0,
                invalid_count: 0,
                updated_at,
            });
            let scope_row = scopes
                .last_mut()
                .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
            add_health_count(scope_row, status, count)?;
            continue;
        };
        if scope_row.updated_at != updated_at {
            scope_row.updated_at = match (scope_row.updated_at, updated_at) {
                (Some(previous), Some(current)) => Some(previous.max(current)),
                (None, current) => current,
                (current, None) => current,
            };
        }
        add_health_count(scope_row, status, count)?;
    }
    let snapshot = AdminCredentialHealthSnapshot {
        observed_at: chrono::Utc::now(),
        last_admin_change_at,
        scopes,
    };
    snapshot
        .validate()
        .then_some(snapshot)
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))
}

#[cfg(feature = "postgres")]
fn add_health_count(
    scope: &mut AdminCredentialHealthScope,
    status: AdminCredentialStatus,
    count: i64,
) -> Result<(), ApplicationError> {
    let target = match status {
        AdminCredentialStatus::Active => &mut scope.active_count,
        AdminCredentialStatus::Disabled => &mut scope.disabled_count,
        AdminCredentialStatus::Invalid => &mut scope.invalid_count,
    };
    *target = target
        .checked_add(count)
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::super::PostgresStore,
    request: AdminCredentialListRequest,
) -> Result<AdminCredentialPage, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启凭据列表事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置凭据列表隔离级别失败"))?;

    let kind = request.kind.map(AdminCredentialKind::as_str);
    let (mail_scope, forum_scope) = world_scopes(request.world, request.kind);
    let mail_scope = mail_scope.as_deref();
    let forum_scope = forum_scope.as_deref();
    let status = request.status.map(AdminCredentialStatus::as_str);
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM public.credential_keys
         WHERE ($1::text IS NULL OR credential_kind = $1)
           AND ($2::text IS NULL OR scope = $2 OR scope = $3)
           AND ($4::text IS NULL OR status = $4)",
    )
    .bind(kind)
    .bind(mail_scope)
    .bind(forum_scope)
    .bind(status)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计 Admin 凭据失败"))?;
    let rows = sqlx::query(
        "SELECT id, credential_kind, scope, label, status, state_version,
                created_at, rotated_at, updated_at
         FROM public.credential_keys
         WHERE ($1::text IS NULL OR credential_kind = $1)
           AND ($2::text IS NULL OR scope = $2 OR scope = $3)
           AND ($4::text IS NULL OR status = $4)
         ORDER BY created_at DESC, id DESC
         LIMIT $5 OFFSET $6",
    )
    .bind(kind)
    .bind(mail_scope)
    .bind(forum_scope)
    .bind(status)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 Admin 凭据列表失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交 Admin 凭据列表事务失败"))?;

    let credentials = rows.iter().map(map_postgres_credential).collect::<Result<Vec<_>, _>>()?;
    let page =
        AdminCredentialPage { credentials, total, limit: request.limit, offset: request.offset };
    page.validate(&request)
        .then_some(page)
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))
}

#[cfg(feature = "postgres")]
async fn find_command_postgres(
    store: &super::super::PostgresStore,
    command_id: Uuid,
    kind: AdminCredentialCommandKind,
    request_hash: RequestHash,
) -> Result<Option<AdminCredentialCommandResult>, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启凭据命令读取事务失败"))?;
    let row = sqlx::query(
        "SELECT command_kind, request_digest, credential_id, result_payload
         FROM public.credential_admin_commands
         WHERE command_id = $1
         FOR UPDATE",
    )
    .bind(command_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    let result = row.map(|row| parse_command(&row, kind, request_hash)).transpose()?;
    transaction.commit().await.map_err(|_| storage_error("提交凭据命令读取事务失败"))?;
    Ok(result)
}

#[cfg(feature = "postgres")]
async fn import_postgres(
    store: &super::super::PostgresStore,
    write: AdminCredentialImportWrite,
) -> Result<AdminCredentialCommandResult, ApplicationError> {
    validate_import_write(&write)?;
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启凭据导入事务失败"))?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, write.command_id).await?;
    if let Some(result) = find_command_in_transaction(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Import,
        write.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(|_| storage_error("提交凭据导入重放事务失败"))?;
        return Ok(result);
    }

    let scope = write.world.scope(write.kind);
    sqlx::query(
        "INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
             fingerprint, status, state_version, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'active', 1, $9, $9)",
    )
    .bind(write.credential_id)
    .bind(write.kind.as_str())
    .bind(&scope)
    .bind(&write.label)
    .bind(write.envelope.ciphertext())
    .bind(write.envelope.nonce())
    .bind(write.envelope.encryption_key_id())
    .bind(write.envelope.fingerprint())
    .bind(write.now)
    .execute(&mut *transaction)
    .await
    .map_err(map_write_error)?;
    let credential = AdminCredential {
        id: write.credential_id,
        kind: write.kind,
        world: write.world,
        label: write.label.clone(),
        status: AdminCredentialStatus::Active,
        state_version: 1,
        created_at: write.now,
        rotated_at: None,
        updated_at: write.now,
    };
    let result = credential_result(credential)?;
    insert_command(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Import,
        write.request_hash,
        write.credential_id,
        &result,
        write.now,
    )
    .await?;
    insert_audit(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Import,
        write.credential_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    transaction.commit().await.map_err(|_| storage_error("提交凭据导入事务失败"))?;
    Ok(result)
}

#[cfg(feature = "postgres")]
async fn mutate_postgres(
    store: &super::super::PostgresStore,
    write: AdminCredentialMutationWrite,
) -> Result<AdminCredentialCommandResult, ApplicationError> {
    validate_mutation_write(&write)?;
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启凭据修改事务失败"))?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, write.command_id).await?;
    if let Some(result) = find_command_in_transaction(
        &mut transaction,
        write.command_id,
        write.action,
        write.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(|_| storage_error("提交凭据修改重放事务失败"))?;
        return Ok(result);
    }
    let before = load_credential_for_update(&mut transaction, write.credential_id).await?;
    if before.state_version != write.expected_state_version {
        return Err(admin_error(AdminCredentialError::VersionConflict {
            current_version: before.state_version,
        }));
    }
    let next_version = write
        .expected_state_version
        .checked_add(1)
        .ok_or_else(|| admin_error(AdminCredentialError::InvalidRequest))?;
    let next_label = write.label.as_deref().unwrap_or(&before.label);
    let next_status = write.status.unwrap_or(before.status);
    if write.action == AdminCredentialCommandKind::Rename && write.label.is_none()
        || write.action == AdminCredentialCommandKind::SetStatus && write.status.is_none()
    {
        return Err(admin_error(AdminCredentialError::InvalidRequest));
    }
    let row = sqlx::query(
        "UPDATE public.credential_keys
         SET label = $2, status = $3, state_version = $4, updated_at = $5,
             rotated_at = CASE WHEN $3 = 'active' THEN rotated_at ELSE COALESCE(rotated_at, $5) END
         WHERE id = $1 AND state_version = $6
         RETURNING id, credential_kind, scope, label, status, state_version,
                   created_at, rotated_at, updated_at",
    )
    .bind(write.credential_id)
    .bind(next_label)
    .bind(next_status.as_str())
    .bind(next_version)
    .bind(write.now)
    .bind(write.expected_state_version)
    .fetch_one(&mut *transaction)
    .await
    .map_err(map_write_error)?;
    let credential = map_postgres_credential(&row)?;
    let result = credential_result(credential)?;
    insert_command(
        &mut transaction,
        write.command_id,
        write.action,
        write.request_hash,
        write.credential_id,
        &result,
        write.now,
    )
    .await?;
    insert_audit(
        &mut transaction,
        write.command_id,
        write.action,
        write.credential_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    transaction.commit().await.map_err(|_| storage_error("提交凭据修改事务失败"))?;
    Ok(result)
}

#[cfg(feature = "postgres")]
async fn delete_postgres(
    store: &super::super::PostgresStore,
    write: AdminCredentialDeleteWrite,
) -> Result<AdminCredentialCommandResult, ApplicationError> {
    validate_delete_write(&write)?;
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启凭据删除事务失败"))?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, write.command_id).await?;
    if let Some(result) = find_command_in_transaction(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Delete,
        write.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(|_| storage_error("提交凭据删除重放事务失败"))?;
        return Ok(result);
    }
    let before = load_credential_for_update(&mut transaction, write.credential_id).await?;
    if before.state_version != write.expected_state_version {
        return Err(admin_error(AdminCredentialError::VersionConflict {
            current_version: before.state_version,
        }));
    }
    let deleted = AdminCredentialDeleteResult {
        credential_id: write.credential_id,
        state_version: write
            .expected_state_version
            .checked_add(1)
            .ok_or_else(|| admin_error(AdminCredentialError::InvalidRequest))?,
    };
    let result = deleted_result(deleted)?;
    insert_command(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Delete,
        write.request_hash,
        write.credential_id,
        &result,
        write.now,
    )
    .await?;
    insert_audit(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Delete,
        write.credential_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    sqlx::query("DELETE FROM public.credential_keys WHERE id = $1 AND state_version = $2")
        .bind(write.credential_id)
        .bind(write.expected_state_version)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
    transaction.commit().await.map_err(|_| storage_error("提交凭据删除事务失败"))?;
    Ok(result)
}

#[cfg(feature = "postgres")]
async fn find_command_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command_id: Uuid,
    kind: AdminCredentialCommandKind,
    request_hash: RequestHash,
) -> Result<Option<AdminCredentialCommandResult>, ApplicationError> {
    let row = sqlx::query(
        "SELECT command_kind, request_digest, credential_id, result_payload
         FROM public.credential_admin_commands
         WHERE command_id = $1
         FOR UPDATE",
    )
    .bind(command_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    row.map(|row| parse_command(&row, kind, request_hash)).transpose()
}

#[cfg(feature = "postgres")]
async fn load_credential_for_update(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    credential_id: Uuid,
) -> Result<AdminCredential, ApplicationError> {
    let row = sqlx::query(
        "SELECT id, credential_kind, scope, label, status, state_version,
                created_at, rotated_at, updated_at
         FROM public.credential_keys
         WHERE id = $1
         FOR UPDATE",
    )
    .bind(credential_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| admin_error(AdminCredentialError::NotFound))?;
    map_postgres_credential(&row)
}

#[cfg(feature = "postgres")]
fn parse_command(
    row: &PgRow,
    kind: AdminCredentialCommandKind,
    request_hash: RequestHash,
) -> Result<AdminCredentialCommandResult, ApplicationError> {
    let stored_kind: String = row.try_get("command_kind").map_err(map_storage_error)?;
    let stored_hash: Vec<u8> = row.try_get("request_digest").map_err(map_storage_error)?;
    if stored_kind != kind.as_str() || stored_hash.as_slice() != request_hash.as_bytes() {
        return Err(admin_error(AdminCredentialError::CommandConflict));
    }
    let credential_id: Option<Uuid> = row.try_get("credential_id").map_err(map_storage_error)?;
    let payload: Json<Value> = row.try_get("result_payload").map_err(map_storage_error)?;
    let payload: StoredCredentialPayload = serde_json::from_value(payload.0)
        .map_err(|_| admin_error(AdminCredentialError::StoredResultCorrupt))?;
    let result =
        AdminCredentialCommandResult { credential: payload.credential, deleted: payload.deleted };
    if !result.validate() {
        return Err(admin_error(AdminCredentialError::StoredResultCorrupt));
    }
    match (&result.credential, &result.deleted, credential_id) {
        (Some(credential), None, Some(stored_id)) if credential.id == stored_id => Ok(result),
        (None, Some(deleted), stored_id)
            if stored_id.is_none_or(|stored_id| stored_id == deleted.credential_id) =>
        {
            Ok(result)
        }
        _ => Err(admin_error(AdminCredentialError::StoredResultCorrupt)),
    }
}

#[cfg(feature = "postgres")]
fn map_postgres_credential(row: &PgRow) -> Result<AdminCredential, ApplicationError> {
    let id: Uuid = row.try_get("id").map_err(map_storage_error)?;
    let kind_text: String = row.try_get("credential_kind").map_err(map_storage_error)?;
    let kind = AdminCredentialKind::parse(&kind_text)
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
    let scope: String = row.try_get("scope").map_err(map_storage_error)?;
    let world = world_from_scope(kind, &scope)
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
    let status_text: String = row.try_get("status").map_err(map_storage_error)?;
    let status = AdminCredentialStatus::parse(&status_text)
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
    let credential = AdminCredential {
        id,
        kind,
        world,
        label: row.try_get("label").map_err(map_storage_error)?,
        status,
        state_version: row.try_get("state_version").map_err(map_storage_error)?,
        created_at: row.try_get("created_at").map_err(map_storage_error)?,
        rotated_at: row.try_get("rotated_at").map_err(map_storage_error)?,
        updated_at: row.try_get("updated_at").map_err(map_storage_error)?,
    };
    credential
        .validate()
        .then_some(credential)
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))
}

#[cfg(feature = "postgres")]
fn credential_result(
    credential: AdminCredential,
) -> Result<AdminCredentialCommandResult, ApplicationError> {
    if !credential.validate() {
        return Err(admin_error(AdminCredentialError::StoredResultCorrupt));
    }
    Ok(AdminCredentialCommandResult { credential: Some(credential), deleted: None })
}

#[cfg(feature = "postgres")]
fn deleted_result(
    deleted: AdminCredentialDeleteResult,
) -> Result<AdminCredentialCommandResult, ApplicationError> {
    if !deleted.validate() {
        return Err(admin_error(AdminCredentialError::StoredResultCorrupt));
    }
    Ok(AdminCredentialCommandResult { credential: None, deleted: Some(deleted) })
}

#[cfg(feature = "postgres")]
async fn insert_command(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command_id: Uuid,
    kind: AdminCredentialCommandKind,
    request_hash: RequestHash,
    credential_id: Uuid,
    result: &AdminCredentialCommandResult,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let payload = StoredCredentialPayload {
        credential: result.credential.clone(),
        deleted: result.deleted.clone(),
    };
    sqlx::query(
        "INSERT INTO public.credential_admin_commands
            (command_id, command_kind, request_digest, credential_id, result_payload, created_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(command_id)
    .bind(kind.as_str())
    .bind(request_hash.as_bytes().to_vec())
    .bind(credential_id)
    .bind(Json(serde_json::to_value(payload)?))
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_write_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command_id: Uuid,
    kind: AdminCredentialCommandKind,
    credential_id: Uuid,
    actor_id: &str,
    request_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
             result, request_id, source, metadata)
         VALUES ($1, $2, 'admin', $3, $4, 'credential', $5, 'success', $6, 'admin', $7)",
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(actor_id)
    .bind(format!("credential.{}", kind.as_str()))
    .bind(credential_id.to_string())
    .bind(request_id)
    .bind(Json(json!({ "command_id": command_id, "command_kind": kind.as_str() })))
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn set_serializable(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), ApplicationError> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn lock_command(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command_id: Uuid,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(command_id.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
fn map_write_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("23505") =>
        {
            admin_error(AdminCredentialError::Conflict)
        }
        _ => map_storage_error(error),
    }
}

#[cfg(feature = "postgres")]
fn map_storage_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("40001") =>
        {
            storage_error("Admin 凭据事务发生序列化冲突")
        }
        _ => storage_error("Admin 凭据存储操作失败"),
    }
}

#[cfg(feature = "postgres")]
fn validate_import_write(write: &AdminCredentialImportWrite) -> Result<(), ApplicationError> {
    if write.command_id.is_nil()
        || write.credential_id.is_nil()
        || write.request_hash.as_bytes() == [0; 32]
        || !valid_text(&write.label, 256)
        || !valid_text(&write.actor_id, 256)
        || write.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || write.now.timestamp_nanos_opt().is_none()
        || write.envelope.ciphertext().len() < 16
        || write.envelope.nonce().len() != 12
    {
        return Err(admin_error(AdminCredentialError::InvalidRequest));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn validate_mutation_write(write: &AdminCredentialMutationWrite) -> Result<(), ApplicationError> {
    let fields_valid = match write.action {
        AdminCredentialCommandKind::Rename => {
            write.label.as_deref().is_some_and(|value| valid_text(value, 256))
                && write.status.is_none()
        }
        AdminCredentialCommandKind::SetStatus => {
            write.label.is_none()
                && write.status.is_some_and(AdminCredentialStatus::is_admin_selectable)
        }
        AdminCredentialCommandKind::Import | AdminCredentialCommandKind::Delete => false,
    };
    if write.command_id.is_nil()
        || write.credential_id.is_nil()
        || write.expected_state_version <= 0
        || write.request_hash.as_bytes() == [0; 32]
        || !valid_text(&write.actor_id, 256)
        || write.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || write.now.timestamp_nanos_opt().is_none()
        || !fields_valid
    {
        return Err(admin_error(AdminCredentialError::InvalidRequest));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn validate_delete_write(write: &AdminCredentialDeleteWrite) -> Result<(), ApplicationError> {
    if write.command_id.is_nil()
        || write.credential_id.is_nil()
        || write.expected_state_version <= 0
        || write.request_hash.as_bytes() == [0; 32]
        || !valid_text(&write.actor_id, 256)
        || write.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || write.now.timestamp_nanos_opt().is_none()
    {
        return Err(admin_error(AdminCredentialError::InvalidRequest));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn world_scopes(
    world: Option<AdminCredentialWorld>,
    kind: Option<AdminCredentialKind>,
) -> (Option<String>, Option<String>) {
    match (world, kind) {
        (None, _) => (None, None),
        (Some(world), Some(kind)) => (Some(world.scope(kind)), None),
        (Some(world), None) => (
            Some(world.scope(AdminCredentialKind::MailCookie)),
            Some(world.scope(AdminCredentialKind::ForumCookie)),
        ),
    }
}

#[cfg(feature = "postgres")]
fn world_from_scope(kind: AdminCredentialKind, scope: &str) -> Option<AdminCredentialWorld> {
    let prefix = format!("{}:", kind.scope_prefix());
    let world = scope.strip_prefix(&prefix)?;
    AdminCredentialWorld::parse(world)
}

#[cfg(feature = "postgres")]
fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}
