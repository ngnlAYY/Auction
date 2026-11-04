//! PostgreSQL Users/MoogleMail 行映射、命令结果和安全事务辅助。

use super::*;
use crate::database::audit_health::AuditWriteResult as _;
use subtle::ConstantTimeEq;

pub(super) struct RegistrationUser {
    pub(super) id: Uuid,
    pub(super) username: String,
    pub(super) status: String,
    pub(super) key_status: String,
    pub(super) username_matches: bool,
}

pub(super) fn registration_user(row: &PgRow) -> Result<RegistrationUser, ApplicationError> {
    Ok(RegistrationUser {
        id: row.try_get("id").map_err(map_storage_error)?,
        username: row.try_get("username").map_err(map_storage_error)?,
        status: row.try_get("status").map_err(map_storage_error)?,
        key_status: row.try_get("key_status").map_err(map_storage_error)?,
        username_matches: row.try_get("username_matches").map_err(map_storage_error)?,
    })
}

pub(super) async fn forum_only_identity(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
             FROM public.forum_bidder_identities
             WHERE user_id = $1 AND registration_state = 'forum_only'
         )",
    )
    .bind(user_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_storage_error)
}

pub(super) async fn persist_user_mail_result(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command_id: CommandId,
    kind: MailUserCommandKind,
    request_hash: RequestHash,
    result: MailUserResult,
    now: DateTime<Utc>,
    action: &str,
) -> Result<(), ApplicationError> {
    result.validate(kind).map_err(ApplicationError::from)?;
    let result_code = result.code.as_str();
    sqlx::query(
        "INSERT INTO public.user_mail_commands
            (command_id, command_kind, request_hash, user_id, result_code, result_metadata, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(command_id.as_uuid())
    .bind(kind.as_str())
    .bind(request_hash.as_bytes().as_slice())
    .bind(result.user_id)
    .bind(result_code)
    .bind(json!({ "result_code": result_code }))
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;

    let entity_id = result.user_id.map_or_else(|| command_id.to_string(), |id| id.to_string());
    let audit_result = match result.code {
        MailUserResultCode::Registered
        | MailUserResultCode::Recovered
        | MailUserResultCode::Rebound
        | MailUserResultCode::Rekeyed
        | MailUserResultCode::Unchanged => "success",
        MailUserResultCode::Blacklisted
        | MailUserResultCode::IdentityConflict
        | MailUserResultCode::InvalidKey
        | MailUserResultCode::NotRegistered => "rejected",
    };
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
             result, request_id, source, metadata)
         VALUES ($1, $2, 'worker', 'mooglemail-worker', $3, 'user_mail', $4,
                 $5, $6, 'worker', $7)",
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(action)
    .bind(entity_id)
    .bind(audit_result)
    .bind(command_id.to_string())
    .bind(json!({ "command_kind": kind.as_str(), "result_code": result_code }))
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_storage_error)?;
    Ok(())
}

pub(super) fn parse_mail_command(
    row: &PgRow,
    expected_kind: MailUserCommandKind,
    request_hash: RequestHash,
) -> Result<MailUserResult, ApplicationError> {
    let kind: String = row.try_get("command_kind").map_err(map_storage_error)?;
    let stored_hash: Vec<u8> = row.try_get("request_hash").map_err(map_storage_error)?;
    let expected_hash = request_hash.as_bytes();
    let hashes_match: bool = stored_hash.as_slice().ct_eq(expected_hash.as_slice()).into();
    if kind != expected_kind.as_str() || !hashes_match {
        return Err(auction_application::MailProcessingError::CommandConflict.into());
    }
    let user_id: Option<Uuid> = row.try_get("user_id").map_err(map_storage_error)?;
    let code: String = row.try_get("result_code").map_err(map_storage_error)?;
    let code = parse_result_code(expected_kind, &code)?;
    MailUserResult { user_id, code }.validate(expected_kind).map_err(ApplicationError::from)
}

fn parse_result_code(
    kind: MailUserCommandKind,
    code: &str,
) -> Result<MailUserResultCode, ApplicationError> {
    let result = match (kind, code) {
        (MailUserCommandKind::Register, "registered") => MailUserResultCode::Registered,
        (MailUserCommandKind::Register, "recovered") => MailUserResultCode::Recovered,
        (MailUserCommandKind::Register, "blacklisted") => MailUserResultCode::Blacklisted,
        (MailUserCommandKind::Register, "identity_conflict") => {
            MailUserResultCode::IdentityConflict
        }
        (MailUserCommandKind::Rebind, "rebound") => MailUserResultCode::Rebound,
        (MailUserCommandKind::Rebind, "blacklisted") => MailUserResultCode::Blacklisted,
        (MailUserCommandKind::Rebind, "identity_conflict") => MailUserResultCode::IdentityConflict,
        (MailUserCommandKind::Rebind, "unchanged") => MailUserResultCode::Unchanged,
        (MailUserCommandKind::Rebind, "invalid_key") => MailUserResultCode::InvalidKey,
        (MailUserCommandKind::Rekey, "rekeyed") => MailUserResultCode::Rekeyed,
        (MailUserCommandKind::Rekey, "blacklisted") => MailUserResultCode::Blacklisted,
        (MailUserCommandKind::Rekey, "identity_conflict") => MailUserResultCode::IdentityConflict,
        (MailUserCommandKind::Rekey, "not_registered") => MailUserResultCode::NotRegistered,
        _ => return Err(repository_error("Users MoogleMail 结果码损坏")),
    };
    Ok(result)
}

pub(super) async fn lock_command(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command_id: CommandId,
) -> Result<(), ApplicationError> {
    crate::database::lock_command_id(transaction, command_id).await.map_err(map_storage_error)
}

pub(super) fn validate_register_write(
    write: &MailUserRegisterWrite,
) -> Result<(), ApplicationError> {
    if write.user_id.is_nil()
        || write.public_key_hash == [0; 32]
        || write.command.command_id.is_nil()
        || write.command.request_hash.as_bytes() == [0; 32]
        || !valid_identity(&write.command.username)
        || write.command.now.timestamp_nanos_opt().is_none()
    {
        return Err(repository_error("Users MoogleMail 注册写入参数无效"));
    }
    Ok(())
}

pub(super) fn validate_rebind_command(
    command: &MailUserRebindCommand,
) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.request_hash.as_bytes() == [0; 32]
        || command.old_key_hash == [0; 32]
        || !valid_identity(&command.next_username)
        || command.history_id.is_nil()
        || command.now.timestamp_nanos_opt().is_none()
    {
        return Err(repository_error("Users MoogleMail 换绑参数无效"));
    }
    Ok(())
}

pub(super) fn validate_rekey_write(write: &MailUserRekeyWrite) -> Result<(), ApplicationError> {
    if write.user_id.is_nil()
        || write.public_key_hash == [0; 32]
        || write.command.command_id.is_nil()
        || write.command.request_hash.as_bytes() == [0; 32]
        || !valid_identity(&write.command.username)
        || write.command.now.timestamp_nanos_opt().is_none()
    {
        return Err(repository_error("Users MoogleMail 轮换参数无效"));
    }
    Ok(())
}

pub(super) fn valid_identity(value: &str) -> bool {
    auction_domain::valid_text(value, 256)
}

pub(super) fn map_storage_error(error: sqlx::Error) -> ApplicationError {
    let message = match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("40001") =>
        {
            "Users MoogleMail 事务发生序列化冲突"
        }
        _ => "Users MoogleMail 存储操作失败",
    };
    crate::database::storage_error_from(error, message)
}
