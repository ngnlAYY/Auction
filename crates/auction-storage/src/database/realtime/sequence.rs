//! Realtime stream 序号分配。
//!
//! 两条路径共用同一张 `realtime_streams` 计数行，但只有一条在业务事务内：
//! - 立即分配（`allocate_stream_sequence`）：拍卖创建、生命周期、结算、Admin 领取等非竞价
//!   写入各自串行，仍在同一事务内取号并写事件。
//! - 延后分配（`allocate_deferred_stream_sequences`）：竞价事实在 Serializable 事务内只写
//!   事件行（`stream_sequence IS NULL`），提交后再由分配器补号。出价事务因此不再读写共享
//!   计数行，同场不同 lot 的重叠出价不会再因这次读写反依赖而以 40001 中止（W12.1 / D5）。
//!
//! 分配器一次只允许一个执行者：以事务级 advisory 锁独占，取不到立即返回 0，由当前持锁者或
//! 下一轮 relay 补齐。序号只来自 `next_sequence` 的单向推进，因此任何并发形状都不会产生缺口。

use chrono::{DateTime, Utc};

/// 单次分配器最多补号的事件数；其余留到下一轮，避免一轮占据过长的行锁等待。
#[cfg(feature = "postgres")]
pub(crate) const DEFERRED_SEQUENCE_BATCH: u16 = 256;

/// relay 每轮最多检查多少个仍有待定事件的 stream。待定窗口通常只有毫秒级，正常为空。
#[cfg(feature = "postgres")]
pub(crate) const DEFERRED_STREAM_SCAN_LIMIT: u16 = 64;

/// 提交后补齐序号的预算；超时只推迟到下一轮 relay，不改变命令结果。
#[cfg(feature = "postgres")]
const DEFERRED_SEQUENCE_BUDGET: std::time::Duration = std::time::Duration::from_millis(250);

/// 旧路径：在业务事务内 upsert 同一 stream 计数行。只保留给非竞价写入与基准对照；
/// 竞价事务调用它会把同场所有 lot 串到同一行上（D5）。
const STREAM_SEQUENCE_UPSERT_SQL: &str =
    "INSERT INTO public.realtime_streams AS stream (stream_key, next_sequence, updated_at)
     VALUES ($1, 2, COALESCE($2, CURRENT_TIMESTAMP))
     ON CONFLICT (stream_key) DO UPDATE
     SET next_sequence = stream.next_sequence + 1,
         updated_at = COALESCE($2, CURRENT_TIMESTAMP)
     RETURNING stream.next_sequence - 1";

/// 提交后分配：扫描 → 计数推进 → 写回在同一条语句内完成，因此不存在空号。
#[cfg(feature = "postgres")]
const DEFERRED_SEQUENCE_ALLOCATION_SQL: &str = "WITH pending AS MATERIALIZED (
         SELECT event.event_id, event.created_at
         FROM public.realtime_events AS event
         WHERE event.stream_key = $1 AND event.stream_sequence IS NULL
         ORDER BY event.created_at, event.event_id
         LIMIT $2
         FOR UPDATE
     ), ranked AS MATERIALIZED (
         SELECT pending.event_id,
                row_number() OVER (ORDER BY pending.created_at, pending.event_id) AS position
         FROM pending
     ), bumped AS (
         UPDATE public.realtime_streams AS stream
         SET next_sequence = stream.next_sequence + (SELECT count(*) FROM ranked),
             updated_at = CURRENT_TIMESTAMP
         WHERE stream.stream_key = $1
         RETURNING stream.next_sequence - (SELECT count(*) FROM ranked) AS first_sequence
     )
     UPDATE public.realtime_events AS event
     SET stream_sequence = bumped.first_sequence + ranked.position - 1
     FROM ranked, bumped
     WHERE event.event_id = ranked.event_id";

/// 只在 `test-db` 下暴露本模块的生产 SQL 原文，保证 W12.1 基准测量的是同一份语句，
/// 而不是测试里另写的副本；默认构建与发布制品不包含该出口。
#[cfg(all(feature = "postgres", feature = "test-db"))]
pub mod benchmark_sql {
    /// 旧路径：事务内 upsert 同一 stream 计数行（基准的 before 分支）。
    pub fn sequence_upsert_sql() -> &'static str {
        super::STREAM_SEQUENCE_UPSERT_SQL
    }

    /// 新路径：提交后按 (created_at, event_id) 顺序补号（基准的 after 分支）。
    pub fn deferred_allocation_sql() -> &'static str {
        super::DEFERRED_SEQUENCE_ALLOCATION_SQL
    }
}

/// 在当前事务内为 stream 分配下一个连续序号，并持有该 stream 行锁直到事务结束。
/// 首次写入时创建 stream 并占用序号 1，已有 stream 则原子递增 `next_sequence`。单条
/// `ON CONFLICT DO UPDATE` 与“补建 → `FOR UPDATE` → 递增”取得同一行锁，Serializable 并发更新
/// 冲突仍报告序列化失败；只减少持锁期间的往返。`updated_at` 为 `None` 时使用事务时间。
///
/// 竞价事件不再走这里：出价事务调用它会把同场所有 lot 串到同一行上（D5）。竞价改用
/// [`allocate_deferred_stream_sequences`] 在提交后补号。
pub(crate) async fn allocate_stream_sequence(
    connection: &mut sqlx::PgConnection,
    stream_key: &str,
    updated_at: Option<DateTime<Utc>>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(STREAM_SEQUENCE_UPSERT_SQL)
        .bind(stream_key)
        .bind(updated_at)
        .fetch_one(connection)
        .await
        .inspect_err(|error| {
            crate::database::pool::observe_sql_error(
                error,
                auction_observability::DatabaseConflictSite::RealtimeSequence,
            );
        })
}

/// 给本流中尚未分配序号的事件按 `(created_at, event_id)` 顺序补上连续序号，返回本轮分配数。
///
/// - 事务级 advisory 锁保证同一 stream 同时只有一个分配者；取不到锁立刻返回 0，不排队等待，
///   因此提交后的快速路径不会把出价响应拖在别人后面。
/// - `pending` 先 `FOR UPDATE` 物化再排序，计数与写回在同一语句内完成：`next_sequence` 只推进
///   `count(*)` 步，分配值等于 `first_sequence + position - 1`，不存在空号。
/// - 内容只追加：UPDATE 只写 `stream_sequence`，其余列由迁移 000086 的触发器拒绝改写。
/// - 事件行的 `(stream_key, stream_sequence)` 唯一约束仍由数据库兜底；重复补号会冲突失败，
///   不会静默覆盖。
#[cfg(feature = "postgres")]
pub(crate) async fn allocate_deferred_stream_sequences(
    connection: &mut sqlx::PgConnection,
    stream_key: &str,
    limit: u16,
) -> Result<u64, sqlx::Error> {
    let lock_name = format!("auction/realtime-sequence/v1/{stream_key}");
    let acquired: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&lock_name)
            .fetch_one(&mut *connection)
            .await?;
    if !acquired {
        return Ok(0);
    }
    let assigned = sqlx::query(DEFERRED_SEQUENCE_ALLOCATION_SQL)
        .bind(stream_key)
        .bind(i64::from(limit))
        .execute(&mut *connection)
        .await
        .inspect_err(|error| {
            crate::database::pool::observe_sql_error(
                error,
                auction_observability::DatabaseConflictSite::RealtimeSequence,
            );
        })?;
    Ok(assigned.rows_affected())
}

/// 列出仍有待定事件的 stream；relay 每轮据此补齐提交后未被分配的序号。
/// 覆盖“提交后进程崩溃”“relay 之外的进程提交竞价事件”等窗口，不依赖提交者仍在运行。
#[cfg(feature = "postgres")]
pub(crate) async fn pending_sequence_streams(
    connection: &mut sqlx::PgConnection,
    prefix: &str,
    limit: u16,
) -> Result<Vec<String>, sqlx::Error> {
    let pattern = format!("{prefix}%");
    sqlx::query_scalar(
        "SELECT event.stream_key
         FROM public.realtime_events AS event
         WHERE event.stream_sequence IS NULL AND event.stream_key LIKE $1
         GROUP BY event.stream_key
         ORDER BY event.stream_key ASC
         LIMIT $2",
    )
    .bind(pattern)
    .bind(i64::from(limit))
    .fetch_all(&mut *connection)
    .await
}

#[cfg(feature = "postgres")]
impl crate::database::PostgresStore {
    /// 在一个独立短事务内补齐给定 stream 的待定序号，返回分配总数。
    ///
    /// 调用者已经提交业务事务；这里失败只意味着“事件暂时不进回放”，不能回滚或改变命令结果。
    pub(crate) async fn allocate_deferred_sequences(
        &self,
        stream_keys: &[String],
    ) -> Result<u64, sqlx::Error> {
        if stream_keys.is_empty() {
            return Ok(0);
        }
        let mut transaction = self.pool().begin_with("BEGIN").await?;
        let mut allocated = 0_u64;
        for stream_key in stream_keys {
            allocated = allocated.saturating_add(
                allocate_deferred_stream_sequences(
                    &mut transaction,
                    stream_key,
                    DEFERRED_SEQUENCE_BATCH,
                )
                .await?,
            );
        }
        transaction.commit().await?;
        Ok(allocated)
    }

    /// 补齐全部仍有待定事件的 stream；relay 每轮调用，作为提交后补号的兜底。
    pub(crate) async fn drain_deferred_sequences(&self, prefix: &str) -> Result<u64, sqlx::Error> {
        let streams = {
            let mut connection = self.pool().acquire().await?;
            pending_sequence_streams(&mut connection, prefix, DEFERRED_STREAM_SCAN_LIMIT).await?
        };
        if streams.is_empty() {
            return Ok(0);
        }
        self.allocate_deferred_sequences(&streams).await
    }
}

/// 提交后补齐序号的快速路径：有界、尽力而为，失败只记录告警，不下沉为命令错误。
/// 序号只是“事件何时进入回放”的投影；未分配的事件会被 relay 的下一轮兜底补齐。
#[cfg(feature = "postgres")]
pub(crate) async fn allocate_deferred_sequences_after_commit(
    store: &crate::database::PostgresStore,
    stream_keys: &[String],
) {
    if stream_keys.is_empty() {
        return;
    }
    match tokio::time::timeout(
        DEFERRED_SEQUENCE_BUDGET,
        store.allocate_deferred_sequences(stream_keys),
    )
    .await
    {
        Ok(Ok(_assigned)) => {}
        Ok(Err(error)) => {
            tracing::warn!(
                error = %error,
                "deferred realtime sequence allocation failed; relay will retry"
            );
        }
        Err(error) => {
            let _ = error;
            tracing::warn!(
                "deferred realtime sequence allocation exceeded its budget; relay will retry"
            );
        }
    }
}

/// W12.1 兜底端口实现：Public relay 每轮把提交后未分配序号的竞价事件补进回放。
#[cfg(feature = "postgres")]
impl auction_application::RealtimeSequenceAllocator for crate::database::DatabaseStore {
    fn allocate_pending_sequences(
        &self,
        prefix: String,
    ) -> auction_application::ApplicationFuture<'_, u64> {
        Box::pin(async move {
            if prefix != "auction:" {
                return Err(auction_application::ApplicationError::Repository(
                    auction_application::RepositoryError::Failed(
                        "实时序号分配只接受 auction: 前缀".to_owned(),
                    ),
                ));
            }
            match self {
                crate::database::DatabaseStore::Postgres(store) => {
                    store.drain_deferred_sequences(&prefix).await.map_err(|error| {
                        crate::database::storage_error_from(error, "补齐实时事件待定序号失败")
                    })
                }
            }
        })
    }
}
