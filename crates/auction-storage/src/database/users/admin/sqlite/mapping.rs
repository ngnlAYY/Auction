//! SQLite Lite Users/Admin 行映射、事务边界和错误处理。

use super::*;

pub(super) fn map_sqlite_user(row: &SqliteRow) -> Result<AdminUser, ApplicationError> {
    let status: String = row.try_get("status").map_err(map_sqlite_error)?;
    let key_status: String = row.try_get("key_status").map_err(map_sqlite_error)?;
    let user = AdminUser {
        id: uuid_from_blob(row.try_get("id").map_err(map_sqlite_error)?)?,
        username: row.try_get("username").map_err(map_sqlite_error)?,
        forum_account_id: row.try_get("forum_account_id").map_err(map_sqlite_error)?,
        status: AdminUserStatus::parse(&status)
            .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))?,
        key_status: auction_application::AdminUserKeyStatus::parse(&key_status)
            .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))?,
        state_version: row.try_get("state_version").map_err(map_sqlite_error)?,
        created_at: parse_sqlite_timestamp(
            row.try_get::<String, _>("created_at").map_err(map_sqlite_error)?,
        )?,
        updated_at: parse_sqlite_timestamp(
            row.try_get::<String, _>("updated_at").map_err(map_sqlite_error)?,
        )?,
    };
    user.validate().then_some(user).ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))
}

pub(super) fn parse_sqlite_command(
    row: &SqliteRow,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
) -> Result<AdminUserCommandResult, ApplicationError> {
    let stored_kind: String = row.try_get("command_kind").map_err(map_sqlite_error)?;
    let stored_hash: Vec<u8> = row.try_get("request_digest").map_err(map_sqlite_error)?;
    if stored_kind != kind.as_str() || stored_hash.as_slice() != request_hash.as_bytes() {
        return Err(admin_error(AdminUserError::CommandConflict));
    }
    let user_id = uuid_from_blob(row.try_get("user_id").map_err(map_sqlite_error)?)?;
    let payload: String = row.try_get("result_payload").map_err(map_sqlite_error)?;
    let payload = serde_json::from_str(&payload)
        .map_err(|_| admin_error(AdminUserError::StoredResultCorrupt))?;
    let response = load_response_envelope(
        row.try_get("response_ciphertext").map_err(map_sqlite_error)?,
        row.try_get("response_nonce").map_err(map_sqlite_error)?,
        row.try_get("response_encryption_key_id").map_err(map_sqlite_error)?,
        row.try_get("response_fingerprint").map_err(map_sqlite_error)?,
    )?;
    parse_payload(payload, user_id, response)
}

pub(super) async fn insert_envelope_sqlite(
    connection: &mut SqliteConnection,
    user_id: Uuid,
    envelope: &MailLoginKeyEnvelope,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO user_login_key_envelopes
            (user_id, ciphertext, nonce, encryption_key_id, fingerprint, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(user_id) DO UPDATE SET ciphertext = excluded.ciphertext,
             nonce = excluded.nonce, encryption_key_id = excluded.encryption_key_id,
             fingerprint = excluded.fingerprint, updated_at = excluded.updated_at",
    )
    .bind(uuid_blob(user_id))
    .bind(envelope.ciphertext())
    .bind(envelope.nonce())
    .bind(envelope.encryption_key_id())
    .bind(envelope.fingerprint())
    .bind(sqlite_timestamp(now))
    .bind(sqlite_timestamp(now))
    .execute(&mut *connection)
    .await
    .map_err(map_sqlite_error)?;
    Ok(())
}

pub(super) async fn insert_command_sqlite(
    connection: &mut SqliteConnection,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
    result: &AdminUserCommandResult,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let response = result.response_envelope.as_ref();
    let payload = serde_json::to_string(&json!({ "user": &result.user }))?;
    sqlx::query(
        "INSERT INTO user_admin_commands
            (command_id, command_kind, request_digest, user_id, result_payload,
             response_ciphertext, response_nonce, response_encryption_key_id,
             response_fingerprint, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_blob(command_id))
    .bind(kind.as_str())
    .bind(request_hash.as_bytes().to_vec())
    .bind(uuid_blob(user_id))
    .bind(payload)
    .bind(response.map(|value| value.ciphertext().to_vec()))
    .bind(response.map(|value| value.nonce().to_vec()))
    .bind(response.map(|value| value.encryption_key_id().to_owned()))
    .bind(response.map(|value| value.fingerprint().to_owned()))
    .bind(sqlite_timestamp(now))
    .execute(&mut *connection)
    .await
    .map_err(map_sqlite_error)?;
    Ok(())
}

pub(super) async fn insert_audit_sqlite(
    connection: &mut SqliteConnection,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    user_id: Uuid,
    actor_id: &str,
    request_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let metadata =
        serde_json::to_string(&json!({ "command_id": command_id, "action": kind.audit_str() }))?;
    sqlx::query(
        "INSERT INTO audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
             result, request_id, source, metadata)
         VALUES (?, ?, 'admin', ?, ?, 'user', ?, 'success', ?, 'admin', ?)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(sqlite_timestamp(now))
    .bind(actor_id)
    .bind(format!("user.{}", kind.audit_str()))
    .bind(user_id.to_string())
    .bind(request_id)
    .bind(metadata)
    .execute(&mut *connection)
    .await
    .record_audit_failure()
    .map_err(map_sqlite_error)?;
    Ok(())
}

pub(super) async fn begin_sqlite_write(
    store: &crate::database::SqliteStore,
) -> Result<sqlx::Transaction<'static, sqlx::Sqlite>, ApplicationError> {
    let connection = store.pool().acquire().await.map_err(map_sqlite_error)?;
    // SQLx 跟踪事务深度并在 Drop 时回滚，覆盖 HTTP 超时和任务取消。
    sqlx::Transaction::begin(connection, Some(sqlx::SqlStr::from_static("BEGIN IMMEDIATE")))
        .await
        .map_err(map_sqlite_error)
}

pub(super) async fn finish_sqlite_write<T>(
    transaction: sqlx::Transaction<'static, sqlx::Sqlite>,
    result: Result<T, ApplicationError>,
) -> Result<T, ApplicationError> {
    match result {
        Ok(value) => transaction.commit().await.map(|()| value).map_err(map_sqlite_error),
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

pub(super) fn uuid_blob(value: Uuid) -> Vec<u8> {
    value.as_bytes().to_vec()
}

pub(super) fn uuid_from_blob(value: Vec<u8>) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&value).map_err(|_| storage_error("Lite Admin 用户 UUID 格式无效"))
}

pub(super) fn sqlite_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

pub(super) fn parse_sqlite_timestamp(value: String) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(&value) {
        return Ok(parsed.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::from_naive_utc_and_offset(parsed, Utc));
        }
    }
    Err(storage_error("Lite Admin 用户时间格式无效"))
}

pub(super) fn map_sqlite_write_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.message().contains("UNIQUE")
                || database_error.message().contains("unique") =>
        {
            admin_error(AdminUserError::Conflict)
        }
        _ => map_sqlite_error(error),
    }
}

pub(super) fn map_sqlite_error(error: sqlx::Error) -> ApplicationError {
    let _ = error;
    storage_error("Lite Admin Users 存储操作失败")
}
