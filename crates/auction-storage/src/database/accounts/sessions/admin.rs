//! Admin 会话的 PostgreSQL 持久化实现。

use auction_application::core::{ApplicationError, SessionError};
use auction_application::public::{AdminSessionIdentity, AdminSessionRecord};
use auction_time::Timestamp;

use crate::database::DatabaseStore;
use crate::database::storage_error_from;

/// 启动时的单语句原子撤销不依赖 actor；更换账号、口令或回退配置都不能复活旧会话。
pub(super) async fn revoke_all_postgres(
    store: &DatabaseStore,
    now: Timestamp,
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

// 先物化带锁的行，再用数据库实时钟判断双截止。排队到过期或撤销之后的请求不能续活。
macro_rules! valid_admin_session {
    ($lock:literal, $sql:literal) => {
        concat!(
            "WITH locked AS MATERIALIZED (
                SELECT token_hash, actor, expires_at, absolute_expires_at
                FROM public.admin_sessions
                WHERE token_hash = $1 AND csrf_hash = $2 AND revoked_at IS NULL
                FOR ",
            $lock,
            "
             ), valid AS MATERIALIZED (
                SELECT * FROM locked
                WHERE LEAST(expires_at, absolute_expires_at) > GREATEST($3, clock_timestamp())
             ) ",
            $sql
        )
    };
}

pub(super) async fn authenticate_postgres(
    store: &DatabaseStore,
    token_hash: [u8; 32],
    csrf_hash: [u8; 32],
    now: Timestamp,
    next_expires_at: Option<Timestamp>,
) -> Result<AdminSessionIdentity, ApplicationError> {
    const INSPECT: &str =
        valid_admin_session!("SHARE", "SELECT actor, expires_at, absolute_expires_at FROM valid");
    // 较晚获得锁的旧请求不能倒退截止，续期也不能越过绝对截止。
    const TOUCH: &str = valid_admin_session!(
        "UPDATE",
        "UPDATE public.admin_sessions AS session
         SET last_activity_at = GREATEST(session.last_activity_at, $3),
             expires_at = LEAST(session.absolute_expires_at, GREATEST(session.expires_at, $4))
         FROM valid
         WHERE session.token_hash = valid.token_hash
         RETURNING session.actor, session.expires_at, session.absolute_expires_at"
    );
    let sql = if next_expires_at.is_some() { TOUCH } else { INSPECT };
    let query = sqlx::query_as::<_, (String, Timestamp, Timestamp)>(sql)
        .bind(token_hash.as_slice())
        .bind(csrf_hash.as_slice())
        .bind(now);
    let query = match next_expires_at {
        Some(deadline) => query.bind(deadline),
        None => query,
    };
    let Some((actor, expires_at, absolute_expires_at)) =
        query
            .fetch_optional(store.pool())
            .await
            .map_err(|error| storage_error_from(error, "校验或续期 Admin 会话失败"))?
    else {
        return Err(SessionError::Unauthenticated.into());
    };
    Ok(AdminSessionIdentity { actor, expires_at, absolute_expires_at })
}

pub(super) async fn revoke_postgres(
    store: &DatabaseStore,
    token_hash: [u8; 32],
    now: Timestamp,
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
