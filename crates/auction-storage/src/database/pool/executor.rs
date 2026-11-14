//! TrackedPool 的 sqlx Executor 实现与 SQL 错误观测。

use auction_observability::DatabaseConflictSite;
use futures_util::TryStreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;
use sqlx::{Database, Either, Execute, Executor, SqlStr};

use super::TrackedPool;

/// 让普通 SQLx 查询也通过追踪 acquisition，而不是只统计显式 `acquire()`。
impl<'c, DB> Executor<'c> for &'c TrackedPool<DB>
where
    DB: Database,
    for<'connection> &'connection mut DB::Connection: Executor<'connection, Database = DB>,
{
    type Database = DB;

    fn fetch_many<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxStream<'e, Result<Either<DB::QueryResult, DB::Row>, sqlx::Error>>
    where
        'c: 'e,
        E: 'q + Execute<'q, Self::Database>,
    {
        let pool = (*self).clone();
        Box::pin(async_stream::try_stream! {
            let mut connection = pool.acquire().await?;
            let mut stream = (&mut *connection).fetch_many(query);
            while let Some(value) = stream.try_next().await? {
                yield value;
            }
        })
    }

    fn fetch_optional<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<DB::Row>, sqlx::Error>>
    where
        'c: 'e,
        E: 'q + Execute<'q, Self::Database>,
    {
        let pool = (*self).clone();
        Box::pin(async move {
            let mut connection = pool.acquire().await?;
            (&mut *connection).fetch_optional(query).await
        })
    }

    fn prepare_with<'e>(
        self,
        sql: SqlStr,
        parameters: &'e [<Self::Database as Database>::TypeInfo],
    ) -> BoxFuture<'e, Result<<Self::Database as Database>::Statement, sqlx::Error>>
    where
        'c: 'e,
    {
        let pool = (*self).clone();
        Box::pin(async move {
            let mut connection = pool.acquire().await?;
            (&mut *connection).prepare_with(sql, parameters).await
        })
    }

    fn describe<'e>(
        self,
        sql: SqlStr,
    ) -> BoxFuture<'e, Result<sqlx::Describe<Self::Database>, sqlx::Error>>
    where
        'c: 'e,
    {
        let pool = (*self).clone();
        Box::pin(async move {
            let mut connection = pool.acquire().await?;
            (&mut *connection).describe(sql).await
        })
    }
}

/// 原 SQLSTATE 的窄投影；冲突谓词本身保持纯函数。
pub(crate) fn observe_sql_error(error: &sqlx::Error, site: DatabaseConflictSite) {
    if let Some(database) = error.as_database_error()
        && let Some(code) = database.code()
    {
        auction_observability::record_database_conflict(&code, site);
    }
}
