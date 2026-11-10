//! Public 会话的固定期限与永久会话持久化；账号偏好命令独立放在子模块。
//! 并发上限满额时不拒绝登录，而在同一事务与用户行锁内注销最久未活动的有效会话。

use crate::database::storage_error_from;

use auction_application::core::{
    ApplicationError, SessionError, TransactionPolicy, TransactionRunner,
};
use auction_application::public::{
    PublicSessionIdentity, PublicSessionPreference, PublicSessionRecord,
};
use chrono::{DateTime, Utc};
use sqlx::{Row, postgres::PgRow};

use super::storage_error;
use crate::database::DatabaseStore;
use crate::database::{PostgresCompetitionTransaction, auction::competition::map_sql_error};

mod preference;

pub(super) use preference::{
    get_postgres as get_preference_postgres, set_postgres as set_preference_postgres,
};

pub(super) async fn create_postgres(
    store: &DatabaseStore,
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
    // Invariant: 并发上限不再拒绝登录，而是腾出名额。用户行锁把同一账号的登录、偏好、
    // 封禁与换钥串行化，因此这里的名额计数与“最久未活动”排序在提交前不会被并发的
    // 登录改变；被注销的会话只写 revoked_at，不删除记录，历史会话事实完整保留。
    if max_public_sessions == 0 {
        return Err(SessionError::SessionLimit.into());
    }
    let active_count: i64 = sqlx::query_scalar(
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
    let evictions = evictions_required(active_count, max_public_sessions);
    if evictions > 0 {
        // 最久未活动优先；并列时按创建时间与摘要排序，保证同一输入总有同一结果。
        // 撤销时间不早于行创建时间，也不早于本次登录的创建时间。
        let revoked = sqlx::query(
            "WITH evicted AS (
                SELECT session.token_hash
                FROM public.public_sessions AS session
                WHERE session.user_id = $1 AND session.revoked_at IS NULL
                  AND ((session.expires_at IS NULL AND session.absolute_expires_at IS NULL)
                    OR (session.expires_at > $2 AND session.absolute_expires_at > $2))
                ORDER BY session.last_activity_at ASC, session.created_at ASC, session.token_hash ASC
                FOR UPDATE
                LIMIT $3
             )
             UPDATE public.public_sessions AS session
             SET revoked_at = GREATEST(session.created_at, $2)
             FROM evicted
             WHERE session.token_hash = evicted.token_hash
               AND session.revoked_at IS NULL
             RETURNING session.token_hash",
        )
        .bind(record.user_id)
        .bind(record.created_at)
        .bind(evictions)
        .fetch_all(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        // 行锁内的计数与排序是权威结果；腾不出足够名额时失败关闭，绝不超出上限写入。
        if i64::try_from(revoked.len()).unwrap_or(i64::MAX) != evictions {
            return Err(SessionError::SessionLimit.into());
        }
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
pub(super) async fn inspect_postgres(
    store: &DatabaseStore,
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

pub(super) async fn touch_postgres(
    store: &DatabaseStore,
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

pub(super) async fn revoke_postgres(
    store: &DatabaseStore,
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

/// 本次登录必须腾出的名额数：超出上限的数量加上本次登录自身占用的一个名额。
/// 未满额时为 0；上限为 0 属于无效配置，不在这里腾位。
fn evictions_required(active_sessions: i64, max_public_sessions: u32) -> i64 {
    if max_public_sessions == 0 {
        return 0;
    }
    let limit = i64::from(max_public_sessions);
    if active_sessions < limit { 0 } else { active_sessions - limit + 1 }
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

#[cfg(test)]
mod tests {
    use super::evictions_required;

    #[test]
    fn evicts_nothing_below_the_limit() {
        assert_eq!(evictions_required(0, 10), 0);
        assert_eq!(evictions_required(9, 10), 0);
        assert_eq!(evictions_required(1, 100), 0);
    }

    #[test]
    fn frees_one_slot_when_exactly_full() {
        assert_eq!(evictions_required(10, 10), 1);
        assert_eq!(evictions_required(1, 1), 1);
    }

    #[test]
    fn frees_every_excess_slot_plus_the_new_session() {
        assert_eq!(evictions_required(13, 10), 4);
        assert_eq!(evictions_required(200, 100), 101);
    }

    #[test]
    fn invalid_zero_limit_never_evicts() {
        // 上限为 0 由配置校验拒绝；存储层不为此腾位，也不越权注销任何会话。
        assert_eq!(evictions_required(3, 0), 0);
    }
}
