//! Public 会话的固定期限与永久会话持久化；账号偏好命令独立放在子模块。

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
    let current_user_version: i64 =
        user.try_get("state_version").map_err(|_| storage_error("读取 Public 会话认证版本失败"))?;
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
            .map_err(|_| storage_error("读取 Public 会话用户名失败"))?,
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
    .map_err(|_| storage_error("复核 Public 会话失败"))?
    .ok_or(SessionError::Unauthenticated)?;
    map_identity(&row)
}

#[cfg(feature = "postgres")]
pub(super) async fn touch_postgres(
    store: &PostgresStore,
    token_hash: [u8; 32],
    now: DateTime<Utc>,
) -> Result<PublicSessionIdentity, ApplicationError> {
    // 外层截止条件依赖已物化的锁定行，连接或行锁等待不能让过期身份再次通过认证。
    let row = sqlx::query(
        "WITH locked AS MATERIALIZED (
            SELECT session.token_hash, session.expires_at, session.absolute_expires_at,
                   account.username
            FROM public.public_sessions AS session
            JOIN public.users AS account ON account.id = session.user_id
            WHERE session.token_hash = $2 AND session.revoked_at IS NULL
              AND account.status = 'active' AND account.key_status = 'active'
            FOR UPDATE OF session
         )
         UPDATE public.public_sessions AS session
         SET last_activity_at = GREATEST(session.last_activity_at, $1)
         FROM locked
         WHERE session.token_hash = locked.token_hash
           AND ((locked.expires_at IS NULL AND locked.absolute_expires_at IS NULL)
             OR (locked.expires_at > GREATEST($1, clock_timestamp())
               AND locked.absolute_expires_at > GREATEST($1, clock_timestamp())))
         RETURNING session.user_id, locked.username, session.expires_at,
                   session.absolute_expires_at",
    )
    .bind(now)
    .bind(token_hash.as_slice())
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("刷新 Public 会话失败"))?;
    let Some(row) = row else {
        return Err(SessionError::Unauthenticated.into());
    };
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
    .map_err(|_| storage_error("撤销 Public 会话失败"))?;
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
