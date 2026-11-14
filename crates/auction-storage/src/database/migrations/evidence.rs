//! 运维只读证据共用的迁移历史与 schema 身份读取；只执行查询，判定、阶段名与错误类型由调用方决定。

use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{Postgres, Row};

use crate::database::PostgresSchemaIdentity;

/// 迁移历史标记表是否存在；迁移从未运行的库没有这张表。
pub(in crate::database) async fn postgres_history_present<'c, E>(
    executor: E,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::Executor<'c, Database = Postgres>,
{
    sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
        .fetch_one(executor)
        .await
}

/// 按版本排序的完整迁移历史；列集合同时满足完整前缀核验与运维摘要。
pub(in crate::database) async fn read_postgres_history<'c, E>(
    executor: E,
) -> Result<Vec<PgRow>, sqlx::Error>
where
    E: sqlx::Executor<'c, Database = Postgres>,
{
    sqlx::query(
        "SELECT version, description, success, checksum
         FROM public._sqlx_migrations
         ORDER BY version",
    )
    .fetch_all(executor)
    .await
}

/// 读取单例 schema 身份；行缺失、应用名不是 `auction` 或基线版本非正时返回 `None`，
/// 调用方一律按身份无效失败关闭。
pub(in crate::database) async fn read_schema_identity<'c, E>(
    executor: E,
) -> Result<Option<PostgresSchemaIdentity>, sqlx::Error>
where
    E: sqlx::Executor<'c, Database = Postgres>,
{
    let row = sqlx::query(
        "SELECT application, baseline_version, installed_at
         FROM public.auction_schema_identity
         WHERE singleton
         LIMIT 2",
    )
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let application: String = row.try_get("application")?;
    let baseline_version: i64 = row.try_get("baseline_version")?;
    let installed_at: DateTime<Utc> = row.try_get("installed_at")?;
    if application != "auction" || baseline_version <= 0 {
        return Ok(None);
    }
    Ok(Some(PostgresSchemaIdentity { application, baseline_version, installed_at }))
}
