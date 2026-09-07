//! Admin 会话的 PostgreSQL/SQLite 持久化实现。

use auction_application::{
    AdminSessionIdentity, AdminSessionRecord, ApplicationError, SessionError,
};
use chrono::{DateTime, Utc};
#[cfg(feature = "sqlite")]
use sqlx::Connection;
use sqlx::Row;

use super::common::storage_error;
#[cfg(feature = "sqlite")]
use super::common::{parse_timestamp, sqlite_timestamp};
#[cfg(feature = "postgres")]
use crate::database::PostgresStore;
#[cfg(feature = "sqlite")]
use crate::database::SqliteStore;

#[cfg(feature = "postgres")]
pub(super) async fn create_postgres(
    store: &PostgresStore,
    record: AdminSessionRecord,
) -> Result<AdminSessionIdentity, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开始 Admin 会话事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置 Admin 会话事务隔离级别失败"))?;
    // 同一 actor 只允许一个未撤销会话；先锁定已有行，再在本事务中轮换旧 token。
    sqlx::query(
        "SELECT actor
         FROM public.admin_sessions
         WHERE actor = $1 AND revoked_at IS NULL
         FOR UPDATE",
    )
    .bind(&record.actor)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("锁定 Admin 会话主体失败"))?;
    sqlx::query(
        "UPDATE public.admin_sessions
         SET revoked_at = $1
         WHERE actor = $2 AND revoked_at IS NULL",
    )
    .bind(record.created_at)
    .bind(&record.actor)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("轮换旧 Admin 会话失败"))?;
    sqlx::query(
        "INSERT INTO public.admin_sessions
            (token_hash, actor, csrf_hash, created_at, last_activity_at, expires_at, absolute_expires_at)
         VALUES ($1, $2, $3, $4, $4, $5, $6)",
    )
    .bind(record.token_hash.as_slice())
    .bind(&record.actor)
    .bind(record.csrf_hash.as_slice())
    .bind(record.created_at)
    .bind(record.expires_at)
    .bind(record.absolute_expires_at)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("创建 Admin 会话失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交 Admin 会话事务失败"))?;
    Ok(AdminSessionIdentity {
        actor: record.actor,
        expires_at: record.expires_at,
        absolute_expires_at: record.absolute_expires_at,
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn touch_postgres(
    store: &PostgresStore,
    token_hash: [u8; 32],
    csrf_hash: [u8; 32],
    now: DateTime<Utc>,
    next_expires_at: DateTime<Utc>,
) -> Result<AdminSessionIdentity, ApplicationError> {
    let row = sqlx::query(
        "UPDATE public.admin_sessions
         SET last_activity_at = GREATEST(last_activity_at, $1),
             expires_at = LEAST(absolute_expires_at, GREATEST(expires_at, $2))
         WHERE token_hash = $3
           AND csrf_hash = $4
           AND revoked_at IS NULL
           AND expires_at > $1
         RETURNING actor, expires_at, absolute_expires_at",
    )
    .bind(now)
    .bind(next_expires_at)
    .bind(token_hash.as_slice())
    .bind(csrf_hash.as_slice())
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("刷新 Admin 会话失败"))?;
    let Some(row) = row else {
        return Err(SessionError::Unauthenticated.into());
    };
    Ok(AdminSessionIdentity {
        actor: row.try_get("actor").map_err(|_| storage_error("读取 Admin 会话主体失败"))?,
        expires_at: row
            .try_get("expires_at")
            .map_err(|_| storage_error("读取 Admin 会话过期时间失败"))?,
        absolute_expires_at: row
            .try_get("absolute_expires_at")
            .map_err(|_| storage_error("读取 Admin 会话绝对过期时间失败"))?,
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn revoke_postgres(
    store: &PostgresStore,
    token_hash: [u8; 32],
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE public.admin_sessions
         SET revoked_at = $1
         WHERE token_hash = $2 AND revoked_at IS NULL",
    )
    .bind(now)
    .bind(token_hash.as_slice())
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("撤销 Admin 会话失败"))?;
    Ok(())
}

#[cfg(feature = "sqlite")]
pub(super) async fn create_sqlite(
    store: &SqliteStore,
    record: AdminSessionRecord,
) -> Result<AdminSessionIdentity, ApplicationError> {
    let mut connection =
        store.pool().acquire().await.map_err(|_| storage_error("获取 Lite Admin 会话连接失败"))?;
    let mut transaction = connection
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| storage_error("开始 Lite Admin 会话事务失败"))?;
    sqlx::query(
        "UPDATE admin_sessions
         SET revoked_at = ?
         WHERE actor = ? AND revoked_at IS NULL",
    )
    .bind(sqlite_timestamp(record.created_at))
    .bind(&record.actor)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("轮换 Lite 旧 Admin 会话失败"))?;
    sqlx::query(
        "INSERT INTO admin_sessions
            (token_hash, actor, csrf_hash, created_at, last_activity_at, expires_at, absolute_expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(record.token_hash.to_vec())
    .bind(&record.actor)
    .bind(record.csrf_hash.to_vec())
    .bind(sqlite_timestamp(record.created_at))
    .bind(sqlite_timestamp(record.created_at))
    .bind(sqlite_timestamp(record.expires_at))
    .bind(sqlite_timestamp(record.absolute_expires_at))
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("创建 Lite Admin 会话失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交 Lite Admin 会话事务失败"))?;
    Ok(AdminSessionIdentity {
        actor: record.actor,
        expires_at: record.expires_at,
        absolute_expires_at: record.absolute_expires_at,
    })
}

#[cfg(feature = "sqlite")]
pub(super) async fn touch_sqlite(
    store: &SqliteStore,
    token_hash: [u8; 32],
    csrf_hash: [u8; 32],
    now: DateTime<Utc>,
    next_expires_at: DateTime<Utc>,
) -> Result<AdminSessionIdentity, ApplicationError> {
    let mut connection =
        store.pool().acquire().await.map_err(|_| storage_error("获取 Lite Admin 会话连接失败"))?;
    let mut transaction = connection
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| storage_error("开始 Lite Admin 会话刷新事务失败"))?;
    let now_text = sqlite_timestamp(now);
    let next_text = sqlite_timestamp(next_expires_at);
    let result = sqlx::query(
        "UPDATE admin_sessions
             SET last_activity_at = CASE WHEN last_activity_at < ? THEN ? ELSE last_activity_at END,
             expires_at = MIN(absolute_expires_at, MAX(expires_at, ?))
         WHERE token_hash = ?
           AND csrf_hash = ?
           AND revoked_at IS NULL
           AND expires_at > ?",
    )
    .bind(&now_text)
    .bind(&now_text)
    .bind(&next_text)
    .bind(token_hash.to_vec())
    .bind(csrf_hash.to_vec())
    .bind(&now_text)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("刷新 Lite Admin 会话失败"))?;
    if result.rows_affected() != 1 {
        return Err(SessionError::Unauthenticated.into());
    }
    let row = sqlx::query(
        "SELECT actor, expires_at, absolute_expires_at
         FROM admin_sessions
         WHERE token_hash = ?",
    )
    .bind(token_hash.to_vec())
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 Lite Admin 会话投影失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交 Lite Admin 会话刷新事务失败"))?;
    Ok(AdminSessionIdentity {
        actor: row.try_get("actor").map_err(|_| storage_error("读取 Lite Admin 会话主体失败"))?,
        expires_at: parse_timestamp(
            row.try_get("expires_at")
                .map_err(|_| storage_error("读取 Lite Admin 会话过期时间失败"))?,
        )?,
        absolute_expires_at: parse_timestamp(
            row.try_get("absolute_expires_at")
                .map_err(|_| storage_error("读取 Lite Admin 会话绝对过期时间失败"))?,
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
        "UPDATE admin_sessions SET revoked_at = ?
         WHERE token_hash = ? AND revoked_at IS NULL",
    )
    .bind(sqlite_timestamp(now))
    .bind(token_hash.to_vec())
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("撤销 Lite Admin 会话失败"))?;
    Ok(())
}
