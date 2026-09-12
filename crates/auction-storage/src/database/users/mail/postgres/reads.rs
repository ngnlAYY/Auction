//! PostgreSQL Users/MoogleMail 幂等命令与密文信封读取。

use super::mapping::{map_storage_error, parse_mail_command, valid_identity};
use super::*;

/// 只选择签发所需的候选 UUID；最终身份由注册写事务重新核对。
pub(crate) async fn find_registration_user_id(
    store: &crate::database::PostgresStore,
    username: &str,
) -> Result<Option<Uuid>, ApplicationError> {
    if !valid_identity(username) {
        return Err(repository_error("Users MoogleMail 注册身份查询参数无效"));
    }
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM public.users WHERE lower(username) = lower($1) ORDER BY id LIMIT 2",
    )
    .bind(username)
    .fetch_all(store.pool())
    .await
    .map_err(map_storage_error)?;
    match ids.as_slice() {
        [] => Ok(None),
        [user_id] if !user_id.is_nil() => Ok(Some(*user_id)),
        _ => Err(repository_error("Users MoogleMail 注册身份不唯一或无效")),
    }
}

pub(crate) async fn find_mail_command_postgres(
    store: &crate::database::PostgresStore,
    command_id: CommandId,
    kind: MailUserCommandKind,
    request_hash: RequestHash,
) -> Result<Option<MailUserResult>, ApplicationError> {
    if command_id.is_nil() || request_hash.as_bytes() == [0; 32] {
        return Err(repository_error("Users MoogleMail 幂等查询参数无效"));
    }
    let row = sqlx::query(
        "SELECT command_kind, request_hash, user_id, result_code
         FROM public.user_mail_commands
         WHERE command_id = $1",
    )
    .bind(command_id.as_uuid())
    .fetch_optional(store.pool())
    .await
    .map_err(map_storage_error)?;
    row.map(|row| parse_mail_command(&row, kind, request_hash)).transpose()
}

pub(crate) async fn load_envelope_postgres(
    store: &crate::database::PostgresStore,
    user_id: Uuid,
) -> Result<MailLoginKeyEnvelope, ApplicationError> {
    let row = sqlx::query(
        "SELECT ciphertext, nonce, encryption_key_id, fingerprint
         FROM public.user_login_key_envelopes
         WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(store.pool())
    .await
    .map_err(map_storage_error)?
    .ok_or(auction_application::MailProcessingError::LoginKeyNotFound)?;
    let ciphertext: Vec<u8> = row.try_get("ciphertext").map_err(map_storage_error)?;
    let nonce: Vec<u8> = row.try_get("nonce").map_err(map_storage_error)?;
    let key_id: String = row.try_get("encryption_key_id").map_err(map_storage_error)?;
    let fingerprint: String = row.try_get("fingerprint").map_err(map_storage_error)?;
    MailLoginKeyEnvelope::new(ciphertext, nonce, key_id, fingerprint)
        .map_err(ApplicationError::from)
}

pub(super) async fn load_mail_command_tx(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command_id: CommandId,
    kind: MailUserCommandKind,
    request_hash: RequestHash,
) -> Result<Option<MailUserResult>, ApplicationError> {
    let row = sqlx::query(
        "SELECT command_kind, request_hash, user_id, result_code
         FROM public.user_mail_commands
         WHERE command_id = $1
         FOR UPDATE",
    )
    .bind(command_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    row.map(|row| parse_mail_command(&row, kind, request_hash)).transpose()
}
