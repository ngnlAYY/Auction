//! Public 会话的 PostgreSQL/SQLite 持久化实现。

use auction_application::{
    ApplicationError, PublicSessionIdentity, PublicSessionRecord, SessionError,
};
use chrono::{DateTime, Utc};
use sqlx::Row;

use super::common::storage_error;
#[cfg(feature = "sqlite")]
use super::common::{parse_timestamp, sqlite_timestamp, uuid_blob, uuid_from_blob};
#[cfg(feature = "postgres")]
use crate::database::PostgresStore;
#[cfg(feature = "sqlite")]
use crate::database::SqliteStore;

#[cfg(feature = "postgres")]
pub(super) async fn create_postgres(
    store: &PostgresStore,
    record: PublicSessionRecord,
    max_public_sessions: u32,
) -> Result<PublicSessionIdentity, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开始 Public 会话事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置 Public 会话事务隔离级别失败"))?;

    let user = sqlx::query(
        "SELECT id, username, game_player_id
         FROM public.users
         WHERE id = $1 AND status = 'active' AND key_status = 'active'
         FOR UPDATE",
    )
    .bind(record.user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("锁定 Public 会话用户失败"))?;
    let Some(user) = user else {
        return Err(SessionError::AccountInactive { public_reason: None }.into());
    };
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM public.public_sessions
         WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > $2",
    )
    .bind(record.user_id)
    .bind(record.created_at)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计 Public 活跃会话失败"))?;
    if count >= i64::from(max_public_sessions) {
        return Err(SessionError::SessionLimit.into());
    }
    sqlx::query(
        "INSERT INTO public.public_sessions
            (token_hash, user_id, created_at, last_activity_at, expires_at, absolute_expires_at)
         VALUES ($1, $2, $3, $3, $4, $5)",
    )
    .bind(record.token_hash.as_slice())
    .bind(record.user_id)
    .bind(record.created_at)
    .bind(record.expires_at)
    .bind(record.absolute_expires_at)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("创建 Public 会话失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交 Public 会话事务失败"))?;

    Ok(PublicSessionIdentity {
        user_id: user.try_get("id").map_err(|_| storage_error("读取 Public 会话用户标识失败"))?,
        username: user
            .try_get("username")
            .map_err(|_| storage_error("读取 Public 会话用户名失败"))?,
        game_player_id: user
            .try_get("game_player_id")
            .map_err(|_| storage_error("读取 Public 会话游戏玩家标识失败"))?,
        expires_at: record.expires_at,
        absolute_expires_at: record.absolute_expires_at,
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn touch_postgres(
    store: &PostgresStore,
    token_hash: [u8; 32],
    now: DateTime<Utc>,
    next_expires_at: DateTime<Utc>,
) -> Result<PublicSessionIdentity, ApplicationError> {
    let row = sqlx::query(
        "UPDATE public.public_sessions AS session
         SET last_activity_at = GREATEST(session.last_activity_at, $1),
             expires_at = LEAST(session.absolute_expires_at, GREATEST(session.expires_at, $2))
         FROM public.users AS account
         WHERE session.token_hash = $3
           AND session.revoked_at IS NULL
           AND session.expires_at > $1
           AND account.id = session.user_id
           AND account.status = 'active'
           AND account.key_status = 'active'
         RETURNING session.user_id, account.username, account.game_player_id, session.expires_at,
                   session.absolute_expires_at",
    )
    .bind(now)
    .bind(next_expires_at)
    .bind(token_hash.as_slice())
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("刷新 Public 会话失败"))?;
    let Some(row) = row else {
        return Err(SessionError::Unauthenticated.into());
    };
    Ok(PublicSessionIdentity {
        user_id: row
            .try_get("user_id")
            .map_err(|_| storage_error("读取 Public 会话用户标识失败"))?,
        username: row
            .try_get("username")
            .map_err(|_| storage_error("读取 Public 会话用户名失败"))?,
        game_player_id: row
            .try_get("game_player_id")
            .map_err(|_| storage_error("读取 Public 会话游戏玩家标识失败"))?,
        expires_at: row
            .try_get("expires_at")
            .map_err(|_| storage_error("读取 Public 会话过期时间失败"))?,
        absolute_expires_at: row
            .try_get("absolute_expires_at")
            .map_err(|_| storage_error("读取 Public 会话绝对过期时间失败"))?,
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn revoke_postgres(
    store: &PostgresStore,
    token_hash: [u8; 32],
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE public.public_sessions
         SET revoked_at = $1
         WHERE token_hash = $2 AND revoked_at IS NULL",
    )
    .bind(now)
    .bind(token_hash.as_slice())
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("撤销 Public 会话失败"))?;
    Ok(())
}

#[cfg(feature = "sqlite")]
pub(super) async fn create_sqlite(
    store: &SqliteStore,
    record: PublicSessionRecord,
    max_public_sessions: u32,
) -> Result<PublicSessionIdentity, ApplicationError> {
    let mut connection =
        store.pool().acquire().await.map_err(|_| storage_error("获取 Lite Public 会话连接失败"))?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await
        .map_err(|_| storage_error("开始 Lite Public 会话事务失败"))?;

    let user = sqlx::query(
        "SELECT id, username, game_player_id
         FROM users
         WHERE id = ? AND status = 'active' AND key_status = 'active'",
    )
    .bind(uuid_blob(record.user_id))
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| storage_error("读取 Lite Public 会话用户失败"))?;
    let Some(user) = user else {
        return Err(SessionError::AccountInactive { public_reason: None }.into());
    };
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM public_sessions
         WHERE user_id = ? AND revoked_at IS NULL AND expires_at > ?",
    )
    .bind(uuid_blob(record.user_id))
    .bind(sqlite_timestamp(record.created_at))
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| storage_error("统计 Lite Public 活跃会话失败"))?;
    if count >= i64::from(max_public_sessions) {
        return Err(SessionError::SessionLimit.into());
    }
    sqlx::query(
        "INSERT INTO public_sessions
            (token_hash, user_id, created_at, last_activity_at, expires_at, absolute_expires_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(record.token_hash.to_vec())
    .bind(uuid_blob(record.user_id))
    .bind(sqlite_timestamp(record.created_at))
    .bind(sqlite_timestamp(record.created_at))
    .bind(sqlite_timestamp(record.expires_at))
    .bind(sqlite_timestamp(record.absolute_expires_at))
    .execute(&mut *connection)
    .await
    .map_err(|_| storage_error("创建 Lite Public 会话失败"))?;
    sqlx::query("COMMIT")
        .execute(&mut *connection)
        .await
        .map_err(|_| storage_error("提交 Lite Public 会话事务失败"))?;

    Ok(PublicSessionIdentity {
        user_id: uuid_from_blob(
            user.try_get("id").map_err(|_| storage_error("读取 Lite 会话用户标识失败"))?,
        )?,
        username: user
            .try_get("username")
            .map_err(|_| storage_error("读取 Lite 会话用户名失败"))?,
        game_player_id: user
            .try_get("game_player_id")
            .map_err(|_| storage_error("读取 Lite 会话游戏玩家标识失败"))?,
        expires_at: record.expires_at,
        absolute_expires_at: record.absolute_expires_at,
    })
}

#[cfg(feature = "sqlite")]
pub(super) async fn touch_sqlite(
    store: &SqliteStore,
    token_hash: [u8; 32],
    now: DateTime<Utc>,
    next_expires_at: DateTime<Utc>,
) -> Result<PublicSessionIdentity, ApplicationError> {
    let mut connection =
        store.pool().acquire().await.map_err(|_| storage_error("获取 Lite Public 会话连接失败"))?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await
        .map_err(|_| storage_error("开始 Lite Public 会话刷新事务失败"))?;
    let now_text = sqlite_timestamp(now);
    let next_text = sqlite_timestamp(next_expires_at);
    let result = sqlx::query(
        "UPDATE public_sessions
         SET last_activity_at = CASE WHEN last_activity_at < ? THEN ? ELSE last_activity_at END,
             expires_at = CASE
                 WHEN absolute_expires_at < ? THEN absolute_expires_at
                 WHEN expires_at < ? THEN ?
                 ELSE expires_at
             END
         WHERE token_hash = ?
           AND revoked_at IS NULL
           AND expires_at > ?
           AND EXISTS (
               SELECT 1 FROM users AS account
               WHERE account.id = public_sessions.user_id
                 AND account.status = 'active'
                 AND account.key_status = 'active'
           )",
    )
    .bind(&now_text)
    .bind(&now_text)
    .bind(&next_text)
    .bind(&now_text)
    .bind(&next_text)
    .bind(token_hash.to_vec())
    .bind(&now_text)
    .execute(&mut *connection)
    .await
    .map_err(|_| storage_error("刷新 Lite Public 会话失败"))?;
    if result.rows_affected() != 1 {
        return Err(SessionError::Unauthenticated.into());
    }
    let row = sqlx::query(
        "SELECT session.user_id, account.username, account.game_player_id,
                session.expires_at, session.absolute_expires_at
         FROM public_sessions AS session
         INNER JOIN users AS account ON account.id = session.user_id
         WHERE session.token_hash = ?",
    )
    .bind(token_hash.to_vec())
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| storage_error("读取 Lite Public 会话投影失败"))?;
    sqlx::query("COMMIT")
        .execute(&mut *connection)
        .await
        .map_err(|_| storage_error("提交 Lite Public 会话刷新事务失败"))?;

    Ok(PublicSessionIdentity {
        user_id: uuid_from_blob(
            row.try_get("user_id").map_err(|_| storage_error("读取 Lite 会话用户标识失败"))?,
        )?,
        username: row.try_get("username").map_err(|_| storage_error("读取 Lite 会话用户名失败"))?,
        game_player_id: row
            .try_get("game_player_id")
            .map_err(|_| storage_error("读取 Lite 会话游戏玩家标识失败"))?,
        expires_at: parse_timestamp(
            row.try_get("expires_at").map_err(|_| storage_error("读取 Lite 会话过期时间失败"))?,
        )?,
        absolute_expires_at: parse_timestamp(
            row.try_get("absolute_expires_at")
                .map_err(|_| storage_error("读取 Lite 会话绝对过期时间失败"))?,
        )?,
    })
}

#[cfg(feature = "sqlite")]
pub(super) async fn revoke_sqlite(
    store: &SqliteStore,
    token_hash: [u8; 32],
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE public_sessions SET revoked_at = ?
         WHERE token_hash = ? AND revoked_at IS NULL",
    )
    .bind(sqlite_timestamp(now))
    .bind(token_hash.to_vec())
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("撤销 Lite Public 会话失败"))?;
    Ok(())
}
