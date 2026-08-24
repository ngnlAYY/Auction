//! Admin 改名历史读取适配器。
//!
//! 改名事实在 PostgreSQL 与 SQLite Lite 中都由追加式 `user_renames` 表拥有。适配器只返回
//! 安全投影，并在同一读取事务中取得总数、数字页和当前用户版本，避免把数据库行直接越过
//! 应用端口。

use auction_application::{
    AdminRenameListRequest, AdminRenamePage, AdminRenameReader, AdminRenameRecord,
    ApplicationError, ApplicationFuture,
};

use super::DatabaseStore;

impl AdminRenameReader for DatabaseStore {
    fn list(&self, request: AdminRenameListRequest) -> ApplicationFuture<'_, AdminRenamePage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(storage_error("Admin 改名历史查询参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => list_sqlite(store, request).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::PostgresStore,
    request: AdminRenameListRequest,
) -> Result<AdminRenamePage, ApplicationError> {
    use chrono::{DateTime, Utc};
    use sqlx::Row;

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启 Admin 改名历史读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置 Admin 改名历史读取隔离级别失败"))?;

    let name = request.name.as_deref().filter(|value| !value.is_empty());
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM public.user_renames
         WHERE ($1::uuid IS NULL OR user_id = $1)
           AND ($2::text IS NULL
                OR strpos(lower(previous_username), lower($2)) > 0
                OR strpos(lower(next_username), lower($2)) > 0)",
    )
    .bind(request.user_id)
    .bind(name)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计 Admin 改名历史失败"))?;

    let rows = sqlx::query(
        "SELECT rename.id, rename.user_id, user.state_version AS user_state_version,
                rename.previous_username, rename.next_username, rename.actor,
                rename.created_at, rename.reverted_at
         FROM public.user_renames AS rename
         JOIN public.users AS user ON user.id = rename.user_id
         WHERE ($1::uuid IS NULL OR rename.user_id = $1)
           AND ($2::text IS NULL
                OR strpos(lower(rename.previous_username), lower($2)) > 0
                OR strpos(lower(rename.next_username), lower($2)) > 0)
         ORDER BY rename.created_at DESC, rename.id DESC
         LIMIT $3 OFFSET $4",
    )
    .bind(request.user_id)
    .bind(name)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 Admin 改名历史失败"))?;

    let entries = rows
        .into_iter()
        .map(|row| {
            let record = AdminRenameRecord {
                id: row.try_get("id").map_err(|_| storage_error("读取改名记录标识失败"))?,
                user_id: row
                    .try_get("user_id")
                    .map_err(|_| storage_error("读取改名用户标识失败"))?,
                user_state_version: row
                    .try_get("user_state_version")
                    .map_err(|_| storage_error("读取改名用户版本失败"))?,
                previous_username: row
                    .try_get("previous_username")
                    .map_err(|_| storage_error("读取改名前用户名失败"))?,
                next_username: row
                    .try_get("next_username")
                    .map_err(|_| storage_error("读取改名后用户名失败"))?,
                actor: row.try_get("actor").map_err(|_| storage_error("读取改名 actor 失败"))?,
                created_at: row
                    .try_get::<DateTime<Utc>, _>("created_at")
                    .map_err(|_| storage_error("读取改名时间失败"))?,
                reverted_at: row
                    .try_get::<Option<DateTime<Utc>>, _>("reverted_at")
                    .map_err(|_| storage_error("读取改名回滚时间失败"))?,
            };
            if record.validate() {
                Ok(record)
            } else {
                Err(storage_error("Admin 改名历史安全投影无效"))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    transaction.commit().await.map_err(|_| storage_error("提交 Admin 改名历史读取事务失败"))?;
    let page = AdminRenamePage { entries, total, limit: request.limit, offset: request.offset };
    if page.validate(&request) {
        Ok(page)
    } else {
        Err(storage_error("Admin 改名历史分页结果无效"))
    }
}

#[cfg(feature = "sqlite")]
async fn list_sqlite(
    store: &super::SqliteStore,
    request: AdminRenameListRequest,
) -> Result<AdminRenamePage, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| storage_error("开启 SQLite Admin 改名历史读取事务失败"))?;
    let user_id = request.user_id.map(|value| value.as_bytes().to_vec());
    let name = request.name.as_deref().filter(|value| !value.is_empty());
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM user_renames
         WHERE (? IS NULL OR user_id = ?)
           AND (? IS NULL
                OR instr(lower(previous_username), lower(?)) > 0
                OR instr(lower(next_username), lower(?)) > 0)",
    )
    .bind(user_id.as_deref())
    .bind(user_id.as_deref())
    .bind(name)
    .bind(name)
    .bind(name)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计 SQLite Admin 改名历史失败"))?;

    let rows = sqlx::query(
        "SELECT rename.id, rename.user_id, user.state_version AS user_state_version,
                rename.previous_username, rename.next_username, rename.actor,
                rename.created_at, rename.reverted_at
         FROM user_renames AS rename
         JOIN users AS user ON user.id = rename.user_id
         WHERE (? IS NULL OR rename.user_id = ?)
           AND (? IS NULL
                OR instr(lower(rename.previous_username), lower(?)) > 0
                OR instr(lower(rename.next_username), lower(?)) > 0)
         ORDER BY datetime(rename.created_at) DESC, rename.id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(user_id.as_deref())
    .bind(user_id.as_deref())
    .bind(name)
    .bind(name)
    .bind(name)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 SQLite Admin 改名历史失败"))?;

    let entries = rows.into_iter().map(map_sqlite_record).collect::<Result<Vec<_>, _>>()?;
    transaction
        .commit()
        .await
        .map_err(|_| storage_error("提交 SQLite Admin 改名历史读取事务失败"))?;
    let page = AdminRenamePage { entries, total, limit: request.limit, offset: request.offset };
    if page.validate(&request) {
        Ok(page)
    } else {
        Err(storage_error("SQLite Admin 改名历史分页结果无效"))
    }
}

#[cfg(feature = "sqlite")]
fn map_sqlite_record(row: sqlx::sqlite::SqliteRow) -> Result<AdminRenameRecord, ApplicationError> {
    use sqlx::Row;

    let id =
        uuid_from_blob(row.try_get("id").map_err(|_| storage_error("读取 SQLite 改名标识失败"))?)?;
    let user_id = uuid_from_blob(
        row.try_get("user_id").map_err(|_| storage_error("读取 SQLite 改名用户标识失败"))?,
    )?;
    let created_at = parse_sqlite_timestamp(
        &row.try_get::<String, _>("created_at")
            .map_err(|_| storage_error("读取 SQLite 改名时间失败"))?,
    )?;
    let reverted_at = row
        .try_get::<Option<String>, _>("reverted_at")
        .map_err(|_| storage_error("读取 SQLite 改名回滚时间失败"))?
        .as_deref()
        .map(parse_sqlite_timestamp)
        .transpose()?;
    let record = AdminRenameRecord {
        id,
        user_id,
        user_state_version: row
            .try_get("user_state_version")
            .map_err(|_| storage_error("读取 SQLite 改名用户版本失败"))?,
        previous_username: row
            .try_get("previous_username")
            .map_err(|_| storage_error("读取 SQLite 改名前用户名失败"))?,
        next_username: row
            .try_get("next_username")
            .map_err(|_| storage_error("读取 SQLite 改名后用户名失败"))?,
        actor: row.try_get("actor").map_err(|_| storage_error("读取 SQLite 改名 actor 失败"))?,
        created_at,
        reverted_at,
    };
    if record.validate() {
        Ok(record)
    } else {
        Err(storage_error("SQLite Admin 改名历史安全投影无效"))
    }
}

#[cfg(feature = "sqlite")]
fn uuid_from_blob(value: Vec<u8>) -> Result<uuid::Uuid, ApplicationError> {
    uuid::Uuid::from_slice(&value).map_err(|_| storage_error("SQLite 改名 UUID 格式无效"))
}

#[cfg(feature = "sqlite")]
fn parse_sqlite_timestamp(value: &str) -> Result<chrono::DateTime<chrono::Utc>, ApplicationError> {
    use chrono::{DateTime, NaiveDateTime, Utc};
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Ok(DateTime::from_naive_utc_and_offset(parsed, Utc));
        }
    }
    Err(storage_error("SQLite 改名时间格式无效"))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}
