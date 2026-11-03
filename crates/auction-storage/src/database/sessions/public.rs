//! Public 会话的固定期限与永久会话持久化；账号偏好命令独立放在子模块。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::{
    ApplicationError, PublicSessionIdentity, PublicSessionPreference, PublicSessionRecord,
    SessionError, TransactionPolicy, TransactionRunner,
};
use chrono::{DateTime, Utc};
use sqlx::{Row, postgres::PgRow};

use super::common::storage_error;
#[cfg(feature = "postgres")]
use crate::database::PostgresStore;
use crate::database::{PostgresCompetitionTransaction, competition::map_sql_error};

mod preference;

pub(super) use preference::{
    get_postgres as get_preference_postgres, set_postgres as set_preference_postgres,
};

#[cfg(feature = "postgres")]
pub(super) async fn create_postgres(
    store: &PostgresStore,
    record: PublicSessionRecord,
    max_public_sessions: u32,
) -> Result<PublicSessionIdentity, ApplicationError> {
    store
        .for_transaction(auction_observability::DatabaseTransactionOwner::PublicIdentity)
        .run(TransactionPolicy::serializable(), |transaction| {
            Box::pin(create_locked(transaction, record, max_public_sessions))
        })
        .await
}

/// 用户锁同时串行化新登录、偏好修改、封禁与换钥；重试复用同一个新 token。
async fn create_locked(
    transaction: &mut PostgresCompetitionTransaction,
    record: PublicSessionRecord,
    max_public_sessions: u32,
) -> Result<PublicSessionIdentity, ApplicationError> {
    let transaction = transaction.transaction_mut()?;

    let user = sqlx::query(
        "SELECT id AS user_id, username, state_version,
                public_session_duration_seconds, public_session_preference_version
         FROM public.users
         WHERE id = $1 AND status = 'active' AND key_status = 'active'
         FOR UPDATE",
    )
    .bind(record.user_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let Some(user) = user else {
        return Err(SessionError::AccountInactive { public_reason: None }.into());
    };
    // 用户行锁一直持有到会话提交；凭据撤销/重签也锁定同一行并永久撤销旧会话。
    // 因此轮换先提交时旧认证不能创建新会话，登录先提交时新会话会被轮换事务撤销。
    let current_user_version: i64 = user
        .try_get("state_version")
        .map_err(|error| storage_error_from(error, "读取 Public 会话认证版本失败"))?;
    if record.authenticated_user_version <= 0
        || record.authenticated_user_version != current_user_version
    {
        return Err(SessionError::InvalidCredentials.into());
    }
    let preference = map_preference(&user)?;
    let expires_at = preference
        .expires_at_from(record.created_at)
        .map_err(|_| storage_error("Public 登录时长偏好无效"))?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM public.public_sessions
         WHERE user_id = $1 AND revoked_at IS NULL
           AND ((expires_at IS NULL AND absolute_expires_at IS NULL)
             OR (expires_at > $2 AND absolute_expires_at > $2))",
    )
    .bind(record.user_id)
    .bind(record.created_at)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if count >= i64::from(max_public_sessions) {
        return Err(SessionError::SessionLimit.into());
    }
    let session = sqlx::query(
        "INSERT INTO public.public_sessions
            (token_hash, user_id, created_at, last_activity_at, expires_at, absolute_expires_at)
         VALUES ($1, $2, $3, $3, $4, $4)
         RETURNING expires_at, absolute_expires_at",
    )
    .bind(record.token_hash.as_slice())
    .bind(record.user_id)
    .bind(record.created_at)
    .bind(expires_at)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    Ok(PublicSessionIdentity {
        user_id: record.user_id,
        username: user
            .try_get("username")
            .map_err(|error| storage_error_from(error, "读取 Public 会话用户名失败"))?,
        expires_at: session.try_get("expires_at").map_err(map_sql_error)?,
        absolute_expires_at: session.try_get("absolute_expires_at").map_err(map_sql_error)?,
    })
}

/// 长连接只读权威会话，不让协议心跳刷新 last_activity_at 或 expires_at。
#[cfg(feature = "postgres")]
pub(super) async fn inspect_postgres(
    store: &PostgresStore,
    token_hash: [u8; 32],
    now: DateTime<Utc>,
) -> Result<PublicSessionIdentity, ApplicationError> {
    // 查询可能在连接池中等过截止；读取时取实时钟，同时保留请求时间作为保守下界。
    let row = sqlx::query(
        "SELECT session.user_id, account.username,
                session.expires_at, session.absolute_expires_at
         FROM public.public_sessions AS session
         JOIN public.users AS account ON account.id = session.user_id
         WHERE session.token_hash = $1
           AND session.revoked_at IS NULL
           AND ((session.expires_at IS NULL AND session.absolute_expires_at IS NULL)
             OR (session.expires_at > GREATEST($2, clock_timestamp())
               AND session.absolute_expires_at > GREATEST($2, clock_timestamp())))
           AND account.status = 'active' AND account.key_status = 'active'",
    )
    .bind(token_hash.as_slice())
    .bind(now)
    .fetch_optional(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "复核 Public 会话失败"))?
    .ok_or(SessionError::Unauthenticated)?;
    map_identity(&row)
}

#[cfg(feature = "postgres")]
pub(super) async fn touch_postgres(
    store: &PostgresStore,
    token_hash: [u8; 32],
    now: DateTime<Utc>,
) -> Result<PublicSessionIdentity, ApplicationError> {
    // 认证谓词与只读 inspect 相同，并按实时钟复核截止，连接池等待不能复活过期身份。
    // Public 期限不随活动续期，`last_activity_at` 只供运维观察：距上次记录不足 60 秒时不写入，
    // 普通读取因此不再逐请求持有会话行锁；需要写入时仍只向前推进且不早于创建时间。
    let row = sqlx::query(
        "WITH current_session AS MATERIALIZED (
            SELECT session.token_hash, session.user_id, account.username,
                   session.expires_at, session.absolute_expires_at,
                   (session.last_activity_at < $1 - interval '60 seconds') AS activity_due
            FROM public.public_sessions AS session
            JOIN public.users AS account ON account.id = session.user_id
            WHERE session.token_hash = $2 AND session.revoked_at IS NULL
              AND ((session.expires_at IS NULL AND session.absolute_expires_at IS NULL)
                OR (session.expires_at > GREATEST($1, clock_timestamp())
                  AND session.absolute_expires_at > GREATEST($1, clock_timestamp())))
              AND account.status = 'active' AND account.key_status = 'active'
         ), recorded_activity AS (
            UPDATE public.public_sessions AS session
            SET last_activity_at = $1
            FROM current_session
            WHERE session.token_hash = current_session.token_hash
              AND session.revoked_at IS NULL
              AND session.last_activity_at < $1 - interval '60 seconds'
         )
         SELECT user_id, username, expires_at, absolute_expires_at, activity_due
         FROM current_session",
    )
    .bind(now)
    .bind(token_hash.as_slice())
    .fetch_optional(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "刷新 Public 会话失败"))?;
    let Some(row) = row else {
        return Err(SessionError::Unauthenticated.into());
    };
    // 活动 UPDATE 可能等待行锁；仅持锁未改行时 PostgreSQL 不保证重新评估 WHERE。
    // 用新语句的快照和实时钟复核期限/撤销，不能把等待前物化的身份作为认证结果。
    if row.try_get::<bool, _>("activity_due").map_err(map_sql_error)? {
        return inspect_postgres(store, token_hash, now).await;
    }
    map_identity(&row)
}

#[cfg(feature = "postgres")]
pub(super) async fn revoke_postgres(
    store: &PostgresStore,
    token_hash: [u8; 32],
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE public.public_sessions
         SET revoked_at = GREATEST(created_at, $1)
         WHERE token_hash = $2 AND revoked_at IS NULL",
    )
    .bind(now)
    .bind(token_hash.as_slice())
    .execute(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "撤销 Public 会话失败"))?;
    Ok(())
}

fn map_identity(row: &PgRow) -> Result<PublicSessionIdentity, ApplicationError> {
    Ok(PublicSessionIdentity {
        user_id: row.try_get("user_id").map_err(map_sql_error)?,
        username: row.try_get("username").map_err(map_sql_error)?,
        expires_at: row.try_get("expires_at").map_err(map_sql_error)?,
        absolute_expires_at: row.try_get("absolute_expires_at").map_err(map_sql_error)?,
    })
}

fn map_preference(row: &PgRow) -> Result<PublicSessionPreference, ApplicationError> {
    let preference = PublicSessionPreference {
        user_id: row.try_get("user_id").map_err(map_sql_error)?,
        duration_seconds: row.try_get("public_session_duration_seconds").map_err(map_sql_error)?,
        version: row.try_get("public_session_preference_version").map_err(map_sql_error)?,
    };
    preference.validate().map_err(|_| storage_error("Public 登录时长偏好记录无效"))?;
    Ok(preference)
}
