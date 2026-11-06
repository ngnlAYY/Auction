//! PostgreSQL 凭据行、命令结果和写入错误映射。

use super::common::*;

use crate::database::audit_health::AuditWriteResult;
use auction_application::{
    AdminCredential, AdminCredentialCommandKind, AdminCredentialCommandResult,
    AdminCredentialDeleteResult, AdminCredentialDeleteWrite, AdminCredentialError,
    AdminCredentialImportWrite, AdminCredentialKind, AdminCredentialMutationWrite,
    AdminCredentialStatus, AdminCredentialWorld, ApplicationError, RequestHash,
};
#[cfg(feature = "postgres")]
pub(super) use auction_domain::valid_text;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::postgres::PgRow;
use sqlx::types::Json;
use uuid::Uuid;

#[cfg(feature = "postgres")]
pub(super) fn parse_command(
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
    // 删除凭据会通过外键 SET NULL 清空关联；摘要和历史 payload 仍是原命令的重放依据。
    // 关联仍存在时必须一致，不能读取当前凭据替换历史结果。
    match (&result.credential, &result.deleted, credential_id) {
        (Some(credential), None, stored_id)
            if stored_id.is_none_or(|stored_id| credential.id == stored_id) =>
        {
            Ok(result)
        }
        (None, Some(deleted), stored_id)
            if stored_id.is_none_or(|stored_id| stored_id == deleted.credential_id) =>
        {
            Ok(result)
        }
        _ => Err(admin_error(AdminCredentialError::StoredResultCorrupt)),
    }
}

#[cfg(feature = "postgres")]
pub(super) fn map_postgres_credential(row: &PgRow) -> Result<AdminCredential, ApplicationError> {
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
        player_name: row.try_get("player_name").map_err(map_storage_error)?,
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
pub(super) fn credential_result(
    credential: AdminCredential,
) -> Result<AdminCredentialCommandResult, ApplicationError> {
    if !credential.validate() {
        return Err(admin_error(AdminCredentialError::StoredResultCorrupt));
    }
    Ok(AdminCredentialCommandResult { credential: Some(credential), deleted: None })
}

#[cfg(feature = "postgres")]
pub(super) fn deleted_result(
    deleted: AdminCredentialDeleteResult,
) -> Result<AdminCredentialCommandResult, ApplicationError> {
    if !deleted.validate() {
        return Err(admin_error(AdminCredentialError::StoredResultCorrupt));
    }
    Ok(AdminCredentialCommandResult { credential: None, deleted: Some(deleted) })
}

#[cfg(feature = "postgres")]
pub(super) async fn insert_command(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
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
    .bind(request_hash.as_bytes().as_slice())
    .bind(credential_id)
    .bind(Json(serde_json::to_value(payload)?))
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_write_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) struct CredentialAuditContext<'a> {
    pub actor_id: &'a str,
    pub request_id: Option<&'a str>,
    pub now: DateTime<Utc>,
    pub operator_identity: Option<&'a auction_application::CliOperatorIdentity>,
}

#[cfg(feature = "postgres")]
pub(super) async fn insert_audit(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command_id: Uuid,
    kind: AdminCredentialCommandKind,
    credential_id: Uuid,
    context: CredentialAuditContext<'_>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
             result, request_id, source, metadata)
         VALUES ($1, $2, 'admin', $3, $4, 'credential', $5, 'success', $6, $8, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(context.now)
    .bind(context.actor_id)
    .bind(format!("credential.{}", kind.as_str()))
    .bind(credential_id.to_string())
    .bind(context.request_id)
    .bind(Json(auction_application::cli_audit_metadata(
        context.operator_identity,
        json!({ "command_id": command_id, "command_kind": kind.as_str() }),
    )))
    .bind(if context.operator_identity.is_some() { "cli" } else { "admin" })
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) async fn lock_command(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command_id: Uuid,
) -> Result<(), ApplicationError> {
    crate::database::lock_command_id(transaction, command_id).await.map_err(map_storage_error)
}

#[cfg(feature = "postgres")]
pub(super) fn map_write_error(error: sqlx::Error) -> ApplicationError {
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
pub(super) fn map_storage_error(error: sqlx::Error) -> ApplicationError {
    let context = match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("40001") =>
        {
            "Admin 凭据事务发生序列化冲突"
        }
        _ => "Admin 凭据存储操作失败",
    };
    crate::database::storage_error_from(error, context)
}

#[cfg(feature = "postgres")]
pub(super) fn validate_import_write(
    write: &AdminCredentialImportWrite,
) -> Result<(), ApplicationError> {
    if write.command_id.is_nil()
        || write.credential_id.is_nil()
        || write.request_hash.as_bytes() == [0; 32]
        || !valid_text(&write.label, 256)
        || !auction_application::valid_credential_player_name(
            write.kind,
            write.player_name.as_deref(),
        )
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
pub(super) fn validate_mutation_write(
    write: &AdminCredentialMutationWrite,
) -> Result<(), ApplicationError> {
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
pub(super) fn validate_delete_write(
    write: &AdminCredentialDeleteWrite,
) -> Result<(), ApplicationError> {
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
pub(super) fn world_scopes(
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
pub(super) fn world_from_scope(
    kind: AdminCredentialKind,
    scope: &str,
) -> Option<AdminCredentialWorld> {
    let prefix = format!("{}:", kind.scope_prefix());
    let world = scope.strip_prefix(&prefix)?;
    AdminCredentialWorld::parse(world)
}
