//! PostgreSQL Users owner 的 MoogleMail 注册、换绑和登录 key 密文读取。
//!
//! 注册与换绑仍只在 PostgreSQL 完整档提供；Admin 用户管理的密文信封读取则同时支持
//! PostgreSQL 和 SQLite Lite。

#[cfg(feature = "postgres")]
use auction_application::MailUserResultCode;
use auction_application::{
    ApplicationError, MailLoginKeyEnvelope, MailLoginKeyEnvelopeReader, MailUserCommandKind,
    MailUserRebindCommand, MailUserRegisterWrite, MailUserResult, MailUserStore, RepositoryError,
    RequestHash,
};
use auction_domain::CommandId;
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde_json::json;
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::postgres::PgRow;
#[cfg(feature = "postgres")]
use subtle::ConstantTimeEq;
use uuid::Uuid;

#[cfg(feature = "postgres")]
use super::super::PostgresStore;
use super::DatabaseStore;

impl MailUserStore for DatabaseStore {
    fn find_mail_command(
        &self,
        _command_id: CommandId,
        _kind: MailUserCommandKind,
        _request_hash: RequestHash,
    ) -> auction_application::ApplicationFuture<'_, Option<MailUserResult>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    find_mail_command_postgres(store, _command_id, _kind, _request_hash).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "Users MoogleMail 命令 owner 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn register_or_recover(
        &self,
        _write: MailUserRegisterWrite,
    ) -> auction_application::ApplicationFuture<'_, MailUserResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => register_postgres(store, _write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "Users MoogleMail 注册 owner 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn rebind_game_identity(
        &self,
        _command: MailUserRebindCommand,
    ) -> auction_application::ApplicationFuture<'_, MailUserResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => rebind_postgres(store, _command).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "Users MoogleMail 换绑 owner 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

impl MailLoginKeyEnvelopeReader for DatabaseStore {
    fn load_login_key_envelope(
        &self,
        user_id: Uuid,
    ) -> auction_application::ApplicationFuture<'_, MailLoginKeyEnvelope> {
        Box::pin(async move {
            if user_id.is_nil() {
                return Err(repository_error("登录 key 信封用户标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => load_envelope_postgres(store, user_id).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => load_envelope_sqlite(store, user_id).await,
            }
        })
    }
}

#[cfg(feature = "sqlite")]
async fn load_envelope_sqlite(
    store: &super::super::SqliteStore,
    user_id: Uuid,
) -> Result<MailLoginKeyEnvelope, ApplicationError> {
    let row = sqlx::query(
        "SELECT ciphertext, nonce, encryption_key_id, fingerprint
         FROM user_login_key_envelopes WHERE user_id = ?",
    )
    .bind(user_id.as_bytes().to_vec())
    .fetch_optional(store.pool())
    .await
    .map_err(|_| repository_error("读取 Lite 登录 key 信封失败"))?
    .ok_or(auction_application::MailProcessingError::LoginKeyNotFound)?;
    let ciphertext: Vec<u8> =
        row.try_get("ciphertext").map_err(|_| repository_error("读取 Lite 登录 key 密文失败"))?;
    let nonce: Vec<u8> =
        row.try_get("nonce").map_err(|_| repository_error("读取 Lite 登录 key nonce 失败"))?;
    let key_id: String = row
        .try_get("encryption_key_id")
        .map_err(|_| repository_error("读取 Lite 登录 key 数据密钥标识失败"))?;
    let fingerprint: String =
        row.try_get("fingerprint").map_err(|_| repository_error("读取 Lite 登录 key 指纹失败"))?;
    MailLoginKeyEnvelope::new(ciphertext, nonce, key_id, fingerprint)
        .map_err(ApplicationError::from)
}

#[cfg(feature = "postgres")]
async fn find_mail_command_postgres(
    store: &PostgresStore,
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

#[cfg(feature = "postgres")]
async fn load_envelope_postgres(
    store: &PostgresStore,
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

#[cfg(feature = "postgres")]
async fn register_postgres(
    store: &PostgresStore,
    write: MailUserRegisterWrite,
) -> Result<MailUserResult, ApplicationError> {
    validate_register_write(&write)?;
    let mut transaction = store.pool().begin().await.map_err(map_storage_error)?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, write.command.command_id).await?;

    if let Some(result) = load_mail_command_tx(
        &mut transaction,
        write.command.command_id,
        MailUserCommandKind::Register,
        write.command.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(result);
    }

    let rows = sqlx::query(
        "SELECT id, username, status, key_status, game_player_id,
                (lower(username) = lower($1)) AS username_matches,
                (game_player_id IS NOT NULL AND lower(game_player_id) = lower($2))
                    AS game_player_matches
         FROM public.users
         WHERE lower(username) = lower($1)
            OR lower(game_player_id) = lower($2)
         ORDER BY id
         FOR UPDATE",
    )
    .bind(&write.command.username)
    .bind(&write.command.game_player_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let result = match rows.as_slice() {
        [] => {
            sqlx::query(
                "INSERT INTO public.users
                    (id, username, status, public_key_hash, game_player_id,
                     key_status, state_version, created_at, updated_at)
                 VALUES ($1, $2, 'active', $3, $4, 'active', 1, $5, $5)",
            )
            .bind(write.user_id)
            .bind(&write.command.username)
            .bind(write.public_key_hash.to_vec())
            .bind(&write.command.game_player_id)
            .bind(write.command.now)
            .execute(&mut *transaction)
            .await
            .map_err(map_storage_error)?;
            insert_login_key_envelope(
                &mut transaction,
                write.user_id,
                &write.envelope,
                write.command.now,
            )
            .await?;
            MailUserResult { user_id: Some(write.user_id), code: MailUserResultCode::Registered }
        }
        [row] => {
            let user = registration_user(row)?;
            if user.username_matches && user.game_player_matches
                || user.game_player_matches
                    && forum_only_identity(&mut transaction, user.id).await?
            {
                if user.status == "blacklisted" {
                    MailUserResult { user_id: Some(user.id), code: MailUserResultCode::Blacklisted }
                } else {
                    let envelope_exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS (
                             SELECT 1 FROM public.user_login_key_envelopes WHERE user_id = $1
                         )",
                    )
                    .bind(user.id)
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(map_storage_error)?;
                    let reissue = user.key_status != "active" || !envelope_exists;
                    let identity_changed = user.username != write.command.username
                        || user.game_player_id.as_deref()
                            != Some(write.command.game_player_id.as_str());
                    if reissue || identity_changed {
                        sqlx::query(
                            "UPDATE public.users
                             SET username = $1,
                                 game_player_id = $2,
                                 public_key_hash = CASE WHEN $3 THEN $4 ELSE public_key_hash END,
                                 key_status = CASE WHEN $3 THEN 'active' ELSE key_status END,
                                 state_version = state_version + 1,
                                 updated_at = $5
                             WHERE id = $6",
                        )
                        .bind(&write.command.username)
                        .bind(&write.command.game_player_id)
                        .bind(reissue)
                        .bind(write.public_key_hash.to_vec())
                        .bind(write.command.now)
                        .bind(user.id)
                        .execute(&mut *transaction)
                        .await
                        .map_err(map_storage_error)?;
                    }
                    if reissue {
                        insert_login_key_envelope(
                            &mut transaction,
                            user.id,
                            &write.envelope,
                            write.command.now,
                        )
                        .await?;
                    }
                    MailUserResult { user_id: Some(user.id), code: MailUserResultCode::Recovered }
                }
            } else {
                MailUserResult { user_id: None, code: MailUserResultCode::IdentityConflict }
            }
        }
        _ => MailUserResult { user_id: None, code: MailUserResultCode::IdentityConflict },
    };
    persist_user_mail_result(
        &mut transaction,
        write.command.command_id,
        MailUserCommandKind::Register,
        write.command.request_hash,
        result,
        write.command.now,
        "mail_user_register",
    )
    .await?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

#[cfg(feature = "postgres")]
async fn rebind_postgres(
    store: &PostgresStore,
    command: MailUserRebindCommand,
) -> Result<MailUserResult, ApplicationError> {
    validate_rebind_command(&command)?;
    let mut transaction = store.pool().begin().await.map_err(map_storage_error)?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, command.command_id).await?;
    if let Some(result) = load_mail_command_tx(
        &mut transaction,
        command.command_id,
        MailUserCommandKind::Rebind,
        command.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(result);
    }

    let rows = sqlx::query(
        "SELECT id, username, status, public_key_hash, game_player_id
         FROM public.users
         WHERE public_key_hash = $1
            OR lower(username) = lower($2)
            OR lower(game_player_id) = lower($3)
         ORDER BY id
         FOR UPDATE",
    )
    .bind(command.old_key_hash.to_vec())
    .bind(&command.next_username)
    .bind(&command.next_game_player_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let source = rows.iter().find(|row| {
        row.try_get::<Vec<u8>, _>("public_key_hash")
            .ok()
            .is_some_and(|hash| hash.as_slice().ct_eq(&command.old_key_hash).into())
    });
    let result = match source {
        None => MailUserResult { user_id: None, code: MailUserResultCode::InvalidKey },
        Some(source) => {
            let user_id: Uuid = source.try_get("id").map_err(map_storage_error)?;
            let status: String = source.try_get("status").map_err(map_storage_error)?;
            if status == "blacklisted" {
                MailUserResult { user_id: Some(user_id), code: MailUserResultCode::Blacklisted }
            } else {
                let conflict = rows.iter().any(|row| {
                    let Ok(other_id) = row.try_get::<Uuid, _>("id") else {
                        return true;
                    };
                    if other_id == user_id {
                        return false;
                    }
                    let username_matches = row
                        .try_get::<String, _>("username")
                        .is_ok_and(|value| value.eq_ignore_ascii_case(&command.next_username));
                    let player_matches = row
                        .try_get::<Option<String>, _>("game_player_id")
                        .ok()
                        .flatten()
                        .is_some_and(|value| {
                            value.eq_ignore_ascii_case(&command.next_game_player_id)
                        });
                    username_matches || player_matches
                });
                if conflict {
                    MailUserResult {
                        user_id: Some(user_id),
                        code: MailUserResultCode::IdentityConflict,
                    }
                } else {
                    let username: String = source.try_get("username").map_err(map_storage_error)?;
                    let game_player_id: Option<String> =
                        source.try_get("game_player_id").map_err(map_storage_error)?;
                    let unchanged = username == command.next_username
                        && game_player_id.as_deref() == Some(command.next_game_player_id.as_str());
                    if unchanged {
                        MailUserResult {
                            user_id: Some(user_id),
                            code: MailUserResultCode::Unchanged,
                        }
                    } else {
                        let affected = sqlx::query(
                            "UPDATE public.users
                             SET username = $1,
                                 game_player_id = $2,
                                 state_version = state_version + 1,
                                 updated_at = $3
                             WHERE id = $4",
                        )
                        .bind(&command.next_username)
                        .bind(&command.next_game_player_id)
                        .bind(command.now)
                        .bind(user_id)
                        .execute(&mut *transaction)
                        .await
                        .map_err(map_storage_error)?;
                        if affected.rows_affected() != 1 {
                            return Err(repository_error("换绑用户更新结果异常"));
                        }
                        sqlx::query(
                            "INSERT INTO public.user_game_identity_history
                                (id, user_id, source_command_id,
                                 previous_username, next_username,
                                 previous_game_player_id, next_game_player_id, created_at)
                             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                        )
                        .bind(command.history_id)
                        .bind(user_id)
                        .bind(command.command_id.as_uuid())
                        .bind(username)
                        .bind(&command.next_username)
                        .bind(game_player_id)
                        .bind(&command.next_game_player_id)
                        .bind(command.now)
                        .execute(&mut *transaction)
                        .await
                        .map_err(map_storage_error)?;
                        MailUserResult { user_id: Some(user_id), code: MailUserResultCode::Rebound }
                    }
                }
            }
        }
    };
    persist_user_mail_result(
        &mut transaction,
        command.command_id,
        MailUserCommandKind::Rebind,
        command.request_hash,
        result,
        command.now,
        "mail_user_rebind",
    )
    .await?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

#[cfg(feature = "postgres")]
struct RegistrationUser {
    id: Uuid,
    username: String,
    status: String,
    key_status: String,
    game_player_id: Option<String>,
    username_matches: bool,
    game_player_matches: bool,
}

#[cfg(feature = "postgres")]
fn registration_user(row: &PgRow) -> Result<RegistrationUser, ApplicationError> {
    Ok(RegistrationUser {
        id: row.try_get("id").map_err(map_storage_error)?,
        username: row.try_get("username").map_err(map_storage_error)?,
        status: row.try_get("status").map_err(map_storage_error)?,
        key_status: row.try_get("key_status").map_err(map_storage_error)?,
        game_player_id: row.try_get("game_player_id").map_err(map_storage_error)?,
        username_matches: row.try_get("username_matches").map_err(map_storage_error)?,
        game_player_matches: row.try_get("game_player_matches").map_err(map_storage_error)?,
    })
}

#[cfg(feature = "postgres")]
async fn forum_only_identity(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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

#[cfg(feature = "postgres")]
async fn insert_login_key_envelope(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    envelope: &MailLoginKeyEnvelope,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.user_login_key_envelopes
            (user_id, ciphertext, nonce, encryption_key_id, fingerprint, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $6)
         ON CONFLICT (user_id) DO UPDATE
         SET ciphertext = EXCLUDED.ciphertext,
             nonce = EXCLUDED.nonce,
             encryption_key_id = EXCLUDED.encryption_key_id,
             fingerprint = EXCLUDED.fingerprint,
             updated_at = EXCLUDED.updated_at",
    )
    .bind(user_id)
    .bind(envelope.ciphertext())
    .bind(envelope.nonce())
    .bind(envelope.encryption_key_id())
    .bind(envelope.fingerprint())
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn persist_user_mail_result(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    .bind(request_hash.as_bytes().to_vec())
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
        | MailUserResultCode::Unchanged => "success",
        MailUserResultCode::Blacklisted
        | MailUserResultCode::IdentityConflict
        | MailUserResultCode::InvalidKey => "rejected",
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
    .map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn load_mail_command_tx(
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

#[cfg(feature = "postgres")]
fn parse_mail_command(
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

#[cfg(feature = "postgres")]
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
        _ => return Err(repository_error("Users MoogleMail 结果码损坏")),
    };
    Ok(result)
}

#[cfg(feature = "postgres")]
async fn lock_command(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command_id: CommandId,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(command_id.to_string())
        .execute(&mut **transaction)
        .await
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
fn validate_register_write(write: &MailUserRegisterWrite) -> Result<(), ApplicationError> {
    if write.user_id.is_nil()
        || write.public_key_hash == [0; 32]
        || write.command.command_id.is_nil()
        || write.command.request_hash.as_bytes() == [0; 32]
        || !valid_identity(&write.command.username)
        || !valid_identity(&write.command.game_player_id)
        || write.command.now.timestamp_nanos_opt().is_none()
    {
        return Err(repository_error("Users MoogleMail 注册写入参数无效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn validate_rebind_command(command: &MailUserRebindCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.request_hash.as_bytes() == [0; 32]
        || command.old_key_hash == [0; 32]
        || !valid_identity(&command.next_username)
        || !valid_identity(&command.next_game_player_id)
        || command.history_id.is_nil()
        || command.now.timestamp_nanos_opt().is_none()
    {
        return Err(repository_error("Users MoogleMail 换绑参数无效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

#[cfg(feature = "postgres")]
fn map_storage_error(error: sqlx::Error) -> ApplicationError {
    let message = match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("40001") =>
        {
            "Users MoogleMail 事务发生序列化冲突"
        }
        _ => "Users MoogleMail 存储操作失败",
    };
    repository_error(message)
}

fn repository_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
