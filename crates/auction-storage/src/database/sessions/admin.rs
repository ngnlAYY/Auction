//! Admin 会话的 PostgreSQL 持久化实现。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::core::{ApplicationError, SessionError};
use auction_application::public::{AdminSessionIdentity, AdminSessionRecord};
use chrono::{DateTime, Utc};
use sqlx::Row;

#[cfg(feature = "postgres")]
use crate::database::DatabaseStore;

/// 启动时的单语句原子撤销不依赖 actor；更换账号、口令或回退配置都不能复活旧会话。
#[cfg(feature = "postgres")]
pub(super) async fn revoke_all_postgres(
    store: &DatabaseStore,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE public.admin_sessions
         SET revoked_at = GREATEST(created_at, $1)
         WHERE revoked_at IS NULL",
    )
    .bind(now)
    .execute(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "撤销启动前的 Admin 会话失败"))?;
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) async fn create_postgres(
    store: &DatabaseStore,
    record: AdminSessionRecord,
) -> Result<AdminSessionIdentity, ApplicationError> {
    store.pool().observe_transaction(
        auction_observability::DatabaseTransactionOwner::Admin,
        async {
    let mut transaction = store
        .pool()
        .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
        .await
        .map_err(|error| storage_error_from(error, "开始 Admin 会话事务失败"))?;
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
    .map_err(|error| storage_error_from(error, "锁定 Admin 会话主体失败"))?;
    sqlx::query(
        "UPDATE public.admin_sessions
         SET revoked_at = GREATEST(created_at, $1)
         WHERE actor = $2 AND revoked_at IS NULL",
    )
    .bind(record.created_at)
    .bind(&record.actor)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "轮换旧 Admin 会话失败"))?;
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
    .map_err(|error| storage_error_from(error, "创建 Admin 会话失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 Admin 会话事务失败"))?;
    Ok(AdminSessionIdentity {
        actor: record.actor,
        expires_at: record.expires_at,
        absolute_expires_at: record.absolute_expires_at,
    })

        },
    ).await
}

#[cfg(feature = "postgres")]
pub(super) async fn touch_postgres(
    store: &DatabaseStore,
    token_hash: [u8; 32],
    csrf_hash: [u8; 32],
    now: DateTime<Utc>,
    next_expires_at: DateTime<Utc>,
) -> Result<AdminSessionIdentity, ApplicationError> {
    // 物化锁定行后再判断截止；只在 UPDATE 谓词换用实时钟，仍可能先判断再等待行锁。
    let row = sqlx::query(
        "WITH locked AS MATERIALIZED (
            SELECT token_hash, expires_at
            FROM public.admin_sessions
            WHERE token_hash = $3 AND csrf_hash = $4 AND revoked_at IS NULL
            FOR UPDATE
         )
         UPDATE public.admin_sessions AS session
         SET last_activity_at = GREATEST(session.last_activity_at, $1),
             expires_at = LEAST(session.absolute_expires_at, GREATEST(session.expires_at, $2))
         FROM locked
         WHERE session.token_hash = locked.token_hash
           AND locked.expires_at > GREATEST($1, clock_timestamp())
         RETURNING session.actor, session.expires_at, session.absolute_expires_at",
    )
    .bind(now)
    .bind(next_expires_at)
    .bind(token_hash.as_slice())
    .bind(csrf_hash.as_slice())
    .fetch_optional(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "刷新 Admin 会话失败"))?;
    let Some(row) = row else {
        return Err(SessionError::Unauthenticated.into());
    };
    Ok(AdminSessionIdentity {
        actor: row
            .try_get("actor")
            .map_err(|error| storage_error_from(error, "读取 Admin 会话主体失败"))?,
        expires_at: row
            .try_get("expires_at")
            .map_err(|error| storage_error_from(error, "读取 Admin 会话过期时间失败"))?,
        absolute_expires_at: row
            .try_get("absolute_expires_at")
            .map_err(|error| storage_error_from(error, "读取 Admin 会话绝对过期时间失败"))?,
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn revoke_postgres(
    store: &DatabaseStore,
    token_hash: [u8; 32],
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE public.admin_sessions
         SET revoked_at = GREATEST(created_at, $1)
         WHERE token_hash = $2 AND revoked_at IS NULL",
    )
    .bind(now)
    .bind(token_hash.as_slice())
    .execute(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "撤销 Admin 会话失败"))?;
    Ok(())
}
