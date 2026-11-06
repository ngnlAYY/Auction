//! PostgreSQL Users/Admin 行映射、幂等结果持久化和错误边界。

use auction_application::CompetitionTransactionError;

use crate::database::audit_health::AuditWriteResult;
use crate::database::users::admin::{admin_error, load_response_envelope, parse_payload};
use auction_application::{
    AdminUser, AdminUserCommandKind, AdminUserCommandResult, AdminUserError, AdminUserStatus,
    ApplicationError, RequestHash,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::postgres::PgRow;
use sqlx::types::Json;
use uuid::Uuid;

pub(super) fn map_postgres_user(row: &PgRow) -> Result<AdminUser, ApplicationError> {
    let status: String = row.try_get("status").map_err(map_storage_error)?;
    let key_status: String = row.try_get("key_status").map_err(map_storage_error)?;
    let user = AdminUser {
        id: row.try_get("id").map_err(map_storage_error)?,
        username: row.try_get("username").map_err(map_storage_error)?,
        forum_account_id: row.try_get("forum_account_id").map_err(map_storage_error)?,
        status: AdminUserStatus::parse(&status)
            .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))?,
        key_status: auction_application::AdminUserKeyStatus::parse(&key_status)
            .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))?,
        state_version: row.try_get("state_version").map_err(map_storage_error)?,
        created_at: row.try_get("created_at").map_err(map_storage_error)?,
        updated_at: row.try_get("updated_at").map_err(map_storage_error)?,
    };
    user.validate().then_some(user).ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))
}

pub(super) fn parse_postgres_command(
    row: &PgRow,
    _command_id: Uuid,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
) -> Result<AdminUserCommandResult, ApplicationError> {
    let stored_kind: String = row.try_get("command_kind").map_err(map_storage_error)?;
    let stored_hash: Vec<u8> = row.try_get("request_digest").map_err(map_storage_error)?;
    if stored_kind != kind.as_str() || stored_hash.as_slice() != request_hash.as_bytes() {
        return Err(admin_error(AdminUserError::CommandConflict));
    }
    let user_id: Uuid = row.try_get("user_id").map_err(map_storage_error)?;
    let payload: Json<Value> = row.try_get("result_payload").map_err(map_storage_error)?;
    let response = load_response_envelope(
        row.try_get("response_ciphertext").map_err(map_storage_error)?,
        row.try_get("response_nonce").map_err(map_storage_error)?,
        row.try_get("response_encryption_key_id").map_err(map_storage_error)?,
        row.try_get("response_fingerprint").map_err(map_storage_error)?,
    )?;
    parse_payload(payload.0, user_id, response)
}

pub(super) async fn insert_command_postgres(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
    result: &AdminUserCommandResult,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let response = result.response_envelope.as_ref();
    sqlx::query(
        "INSERT INTO public.user_admin_commands
            (command_id, command_kind, request_digest, user_id, result_payload,
             response_ciphertext, response_nonce, response_encryption_key_id,
             response_fingerprint, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(command_id)
    .bind(kind.as_str())
    .bind(request_hash.as_bytes().as_slice())
    .bind(user_id)
    .bind(Json(json!({ "user": &result.user })))
    .bind(response.map(|value| value.ciphertext()))
    .bind(response.map(|value| value.nonce()))
    .bind(response.map(|value| value.encryption_key_id()))
    .bind(response.map(|value| value.fingerprint()))
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    Ok(())
}

pub(super) async fn insert_audit_postgres(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    user_id: Uuid,
    actor_id: &str,
    request_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
             result, request_id, source, metadata)
         VALUES ($1, $2, 'admin', $3, $4, 'user', $5, 'success', $6, 'admin', $7)",
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(actor_id)
    .bind(format!("user.{}", kind.audit_str()))
    .bind(user_id.to_string())
    .bind(request_id)
    .bind(Json(json!({ "command_id": command_id, "command_kind": kind.audit_str() })))
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_storage_error)?;
    Ok(())
}

pub(super) async fn lock_command(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command_id: Uuid,
) -> Result<(), ApplicationError> {
    crate::database::lock_command_id(transaction, command_id).await.map_err(map_storage_error)
}

fn map_postgres_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("23505") =>
        {
            admin_error(AdminUserError::Conflict)
        }
        _ => map_storage_error(error),
    }
}

pub(super) fn map_admin_write_error(error: sqlx::Error) -> ApplicationError {
    map_postgres_error(error)
}

pub(super) fn map_storage_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        _ if crate::database::observed_serialization_conflict(&error) => {
            // 不把可重试冲突降成字符串；事务 owner 负责保留原命令并执行有界重试。
            CompetitionTransactionError::SerializationConflict.into()
        }
        _ => crate::database::storage_error_from(error, "Admin Users 存储操作失败"),
    }
}
