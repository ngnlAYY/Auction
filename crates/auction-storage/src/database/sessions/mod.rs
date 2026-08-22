//! sessions owner 的 Public 会话持久化适配器。
//!
//! PostgreSQL 使用 Serializable 事务加用户行锁保护并发会话上限；SQLite Lite 使用
//! `BEGIN IMMEDIATE` 保证创建和刷新不会拆成多个写者。Redis 不参与会话真值。

use auction_application::{
    AdminSessionIdentity, AdminSessionRecord, AdminSessionStore, ApplicationError,
    ApplicationFuture, PublicSessionIdentity, PublicSessionRecord, PublicSessionStore,
    RepositoryError, SessionError,
};
#[cfg(feature = "sqlite")]
use chrono::NaiveDateTime;
use chrono::{DateTime, Utc};
use sqlx::Row;
#[cfg(feature = "sqlite")]
use uuid::Uuid;

use super::DatabaseStore;

impl AdminSessionStore for DatabaseStore {
    fn create_admin(
        &self,
        record: AdminSessionRecord,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => create_admin_postgres(store, record).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => create_admin_sqlite(store, record).await,
            }
        })
    }

    fn touch_admin(
        &self,
        token_hash: [u8; 32],
        csrf_hash: [u8; 32],
        now: DateTime<Utc>,
        next_expires_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    touch_admin_postgres(store, token_hash, csrf_hash, now, next_expires_at).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    touch_admin_sqlite(store, token_hash, csrf_hash, now, next_expires_at).await
                }
            }
        })
    }

    fn revoke_admin(&self, token_hash: [u8; 32], now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    revoke_admin_postgres(store, token_hash, now).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => revoke_admin_sqlite(store, token_hash, now).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn create_admin_postgres(
    store: &super::PostgresStore,
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
async fn touch_admin_postgres(
    store: &super::PostgresStore,
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
async fn revoke_admin_postgres(
    store: &super::PostgresStore,
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
async fn create_admin_sqlite(
    store: &super::SqliteStore,
    record: AdminSessionRecord,
) -> Result<AdminSessionIdentity, ApplicationError> {
    let mut connection =
        store.pool().acquire().await.map_err(|_| storage_error("获取 Lite Admin 会话连接失败"))?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await
        .map_err(|_| storage_error("开始 Lite Admin 会话事务失败"))?;
    sqlx::query(
        "UPDATE admin_sessions
         SET revoked_at = ?
         WHERE actor = ? AND revoked_at IS NULL",
    )
    .bind(sqlite_timestamp(record.created_at))
    .bind(&record.actor)
    .execute(&mut *connection)
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
    .execute(&mut *connection)
    .await
    .map_err(|_| storage_error("创建 Lite Admin 会话失败"))?;
    sqlx::query("COMMIT")
        .execute(&mut *connection)
        .await
        .map_err(|_| storage_error("提交 Lite Admin 会话事务失败"))?;
    Ok(AdminSessionIdentity {
        actor: record.actor,
        expires_at: record.expires_at,
        absolute_expires_at: record.absolute_expires_at,
    })
}

#[cfg(feature = "sqlite")]
async fn touch_admin_sqlite(
    store: &super::SqliteStore,
    token_hash: [u8; 32],
    csrf_hash: [u8; 32],
    now: DateTime<Utc>,
    next_expires_at: DateTime<Utc>,
) -> Result<AdminSessionIdentity, ApplicationError> {
    let mut connection =
        store.pool().acquire().await.map_err(|_| storage_error("获取 Lite Admin 会话连接失败"))?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await
        .map_err(|_| storage_error("开始 Lite Admin 会话刷新事务失败"))?;
    let now_text = sqlite_timestamp(now);
    let next_text = sqlite_timestamp(next_expires_at);
    let result = sqlx::query(
        "UPDATE admin_sessions
         SET last_activity_at = CASE WHEN last_activity_at < ? THEN ? ELSE last_activity_at END,
             expires_at = CASE
                 WHEN absolute_expires_at < ? THEN absolute_expires_at
                 WHEN expires_at < ? THEN ?
                 ELSE expires_at
             END
         WHERE token_hash = ?
           AND csrf_hash = ?
           AND revoked_at IS NULL
           AND expires_at > ?",
    )
    .bind(&now_text)
    .bind(&now_text)
    .bind(&next_text)
    .bind(&now_text)
    .bind(&next_text)
    .bind(token_hash.to_vec())
    .bind(csrf_hash.to_vec())
    .bind(&now_text)
    .execute(&mut *connection)
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
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| storage_error("读取 Lite Admin 会话投影失败"))?;
    sqlx::query("COMMIT")
        .execute(&mut *connection)
        .await
        .map_err(|_| storage_error("提交 Lite Admin 会话刷新事务失败"))?;
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
async fn revoke_admin_sqlite(
    store: &super::SqliteStore,
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

impl PublicSessionStore for DatabaseStore {
    fn create_public(
        &self,
        record: PublicSessionRecord,
        max_public_sessions: u32,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    create_postgres(store, record, max_public_sessions).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    create_sqlite(store, record, max_public_sessions).await
                }
            }
        })
    }

    fn touch_public(
        &self,
        token_hash: [u8; 32],
        now: DateTime<Utc>,
        next_expires_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    touch_postgres(store, token_hash, now, next_expires_at).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    touch_sqlite(store, token_hash, now, next_expires_at).await
                }
            }
        })
    }

    fn revoke_public(&self, token_hash: [u8; 32], now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => revoke_postgres(store, token_hash, now).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => revoke_sqlite(store, token_hash, now).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn create_postgres(
    store: &super::PostgresStore,
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
        "SELECT id, username
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
        expires_at: record.expires_at,
        absolute_expires_at: record.absolute_expires_at,
    })
}

#[cfg(feature = "postgres")]
async fn touch_postgres(
    store: &super::PostgresStore,
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
         RETURNING session.user_id, account.username, session.expires_at,
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
        expires_at: row
            .try_get("expires_at")
            .map_err(|_| storage_error("读取 Public 会话过期时间失败"))?,
        absolute_expires_at: row
            .try_get("absolute_expires_at")
            .map_err(|_| storage_error("读取 Public 会话绝对过期时间失败"))?,
    })
}

#[cfg(feature = "postgres")]
async fn revoke_postgres(
    store: &super::PostgresStore,
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
async fn create_sqlite(
    store: &super::SqliteStore,
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
        "SELECT id, username
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
        expires_at: record.expires_at,
        absolute_expires_at: record.absolute_expires_at,
    })
}

#[cfg(feature = "sqlite")]
async fn touch_sqlite(
    store: &super::SqliteStore,
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
        "SELECT session.user_id, account.username, session.expires_at, session.absolute_expires_at
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
async fn revoke_sqlite(
    store: &super::SqliteStore,
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

#[cfg(feature = "sqlite")]
fn uuid_blob(id: Uuid) -> Vec<u8> {
    id.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
fn uuid_from_blob(bytes: Vec<u8>) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error("Lite 会话用户标识格式无效"))
}

#[cfg(feature = "sqlite")]
fn sqlite_timestamp(value: DateTime<Utc>) -> String {
    value.naive_utc().format("%Y-%m-%d %H:%M:%S%.f").to_string()
}

#[cfg(feature = "sqlite")]
fn parse_timestamp(value: String) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error("Lite 会话时间格式无效"))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
