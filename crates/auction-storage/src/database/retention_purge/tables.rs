//! 清理执行只允许的代码内闭集表与固定 SQL。表名、谓词和排序都在编译期写死，
//! 政策只能引用规则与期限，不能扩展删除范围；审计、资金、业务历史与身份表不在集合内。

use chrono::{DateTime, Utc};
use sqlx::Row;

use super::model::RetentionPurgeError;
use crate::database::TrackedPool;

/// 允许执行删除的运行态表；顺序与政策 v2 的数据库行族一致。
pub(super) const PURGEABLE_TABLES: [&str; 5] = [
    "service_heartbeats",
    "realtime_events",
    "realtime_failures",
    "public_sessions",
    "admin_sessions",
];

/// 计算某表候选边界的规则；会话使用 30 天宽限规则，已注销行不依赖边界。
pub(super) fn cutoff_rule_id(table: &str) -> Option<&'static str> {
    match table {
        "service_heartbeats" => Some("service_heartbeat_expired_grace"),
        "realtime_events" => Some("realtime_event_age"),
        "realtime_failures" => Some("realtime_resolved_failure_age"),
        "public_sessions" => Some("public_session_expired_grace"),
        "admin_sessions" => Some("admin_session_expired_grace"),
        _ => None,
    }
}

/// 单批删除语句；`$1` 为快照时间减期限，`$2` 为本批行数上限。
/// 每批独立事务提交，失败时已提交批次保留并在回执中精确计数。
pub(super) fn delete_statement(table: &str) -> Option<&'static str> {
    match table {
        "service_heartbeats" => Some(
            "WITH batch AS (
                 SELECT t.ctid AS row_id
                 FROM public.service_heartbeats AS t
                 WHERE t.expires_at <= $1
                 ORDER BY t.expires_at, t.service_name, t.instance_id
                 LIMIT $2
             ), removed AS (
                 DELETE FROM public.service_heartbeats AS t
                 USING batch
                 WHERE t.ctid = batch.row_id
                 RETURNING pg_column_size(t) AS row_bytes
             )
             SELECT COUNT(*)::bigint AS deleted_rows,
                    COALESCE(SUM(row_bytes), 0)::bigint AS deleted_bytes
             FROM removed",
        ),
        // 被失败记录或重放关系引用的事件受 RESTRICT 外键阻断；删除谓词跳过它们，
        // 不捕获外键错误、也不先删除失败或重放事实。
        "realtime_events" => Some(
            "WITH batch AS (
                 SELECT t.ctid AS row_id
                 FROM public.realtime_events AS t
                 WHERE t.created_at <= $1
                   AND NOT EXISTS (
                       SELECT 1 FROM public.realtime_failures AS failure
                       WHERE failure.event_id = t.event_id
                          OR failure.replay_event_id = t.event_id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM public.realtime_events AS replay
                       WHERE replay.replay_of_event_id = t.event_id
                   )
                 ORDER BY t.created_at, t.event_id
                 LIMIT $2
             ), removed AS (
                 DELETE FROM public.realtime_events AS t
                 USING batch
                 WHERE t.ctid = batch.row_id
                 RETURNING pg_column_size(t) AS row_bytes
             )
             SELECT COUNT(*)::bigint AS deleted_rows,
                    COALESCE(SUM(row_bytes), 0)::bigint AS deleted_bytes
             FROM removed",
        ),
        // 未解决（resolved_at IS NULL）的失败记录永不进入删除谓词。
        "realtime_failures" => Some(
            "WITH batch AS (
                 SELECT t.ctid AS row_id
                 FROM public.realtime_failures AS t
                 WHERE t.resolved_at IS NOT NULL AND t.resolved_at <= $1
                 ORDER BY t.resolved_at, t.id
                 LIMIT $2
             ), removed AS (
                 DELETE FROM public.realtime_failures AS t
                 USING batch
                 WHERE t.ctid = batch.row_id
                 RETURNING pg_column_size(t) AS row_bytes
             )
             SELECT COUNT(*)::bigint AS deleted_rows,
                    COALESCE(SUM(row_bytes), 0)::bigint AS deleted_bytes
             FROM removed",
        ),
        // 已注销行立即候选；未注销行只在有效截止过期且超过 30 天宽限后候选。
        // expires_at 为 NULL 的永久会话只在已注销时出现。
        "public_sessions" => Some(
            "WITH batch AS (
                 SELECT t.ctid AS row_id
                 FROM public.public_sessions AS t
                 WHERE t.revoked_at IS NOT NULL
                    OR (t.revoked_at IS NULL AND t.expires_at IS NOT NULL AND t.expires_at <= $1)
                 ORDER BY COALESCE(t.revoked_at, t.expires_at), t.token_hash
                 LIMIT $2
             ), removed AS (
                 DELETE FROM public.public_sessions AS t
                 USING batch
                 WHERE t.ctid = batch.row_id
                 RETURNING pg_column_size(t) AS row_bytes
             )
             SELECT COUNT(*)::bigint AS deleted_rows,
                    COALESCE(SUM(row_bytes), 0)::bigint AS deleted_bytes
             FROM removed",
        ),
        "admin_sessions" => Some(
            "WITH batch AS (
                 SELECT t.ctid AS row_id
                 FROM public.admin_sessions AS t
                 WHERE t.revoked_at IS NOT NULL
                    OR (t.revoked_at IS NULL AND t.expires_at IS NOT NULL AND t.expires_at <= $1)
                 ORDER BY COALESCE(t.revoked_at, t.expires_at), t.token_hash
                 LIMIT $2
             ), removed AS (
                 DELETE FROM public.admin_sessions AS t
                 USING batch
                 WHERE t.ctid = batch.row_id
                 RETURNING pg_column_size(t) AS row_bytes
             )
             SELECT COUNT(*)::bigint AS deleted_rows,
                    COALESCE(SUM(row_bytes), 0)::bigint AS deleted_bytes
             FROM removed",
        ),
        _ => None,
    }
}

/// 逐表剩余候选计数；与删除谓词一一对应，事件额外报告被外键阻断的行数。
pub(super) fn remaining_statement(table: &str) -> Option<&'static str> {
    match table {
        "service_heartbeats" => Some(
            "SELECT COUNT(*)::bigint AS remaining_rows, 0::bigint AS blocked_rows
             FROM public.service_heartbeats AS t
             WHERE t.expires_at <= $1",
        ),
        "realtime_events" => Some(
            "SELECT COUNT(*) FILTER (
                        WHERE NOT EXISTS (
                            SELECT 1 FROM public.realtime_failures AS failure
                            WHERE failure.event_id = t.event_id
                               OR failure.replay_event_id = t.event_id
                        )
                          AND NOT EXISTS (
                            SELECT 1 FROM public.realtime_events AS replay
                            WHERE replay.replay_of_event_id = t.event_id
                        )
                    )::bigint AS remaining_rows,
                    COUNT(*) FILTER (
                        WHERE EXISTS (
                            SELECT 1 FROM public.realtime_failures AS failure
                            WHERE failure.event_id = t.event_id
                               OR failure.replay_event_id = t.event_id
                        )
                           OR EXISTS (
                            SELECT 1 FROM public.realtime_events AS replay
                            WHERE replay.replay_of_event_id = t.event_id
                        )
                    )::bigint AS blocked_rows
             FROM public.realtime_events AS t
             WHERE t.created_at <= $1",
        ),
        "realtime_failures" => Some(
            "SELECT COUNT(*)::bigint AS remaining_rows, 0::bigint AS blocked_rows
             FROM public.realtime_failures AS t
             WHERE t.resolved_at IS NOT NULL AND t.resolved_at <= $1",
        ),
        "public_sessions" => Some(
            "SELECT COUNT(*)::bigint AS remaining_rows, 0::bigint AS blocked_rows
             FROM public.public_sessions AS t
             WHERE t.revoked_at IS NOT NULL
                OR (t.revoked_at IS NULL AND t.expires_at IS NOT NULL AND t.expires_at <= $1)",
        ),
        "admin_sessions" => Some(
            "SELECT COUNT(*)::bigint AS remaining_rows, 0::bigint AS blocked_rows
             FROM public.admin_sessions AS t
             WHERE t.revoked_at IS NOT NULL
                OR (t.revoked_at IS NULL AND t.expires_at IS NOT NULL AND t.expires_at <= $1)",
        ),
        _ => None,
    }
}

pub(super) struct RemainingCandidates {
    pub(super) remaining: i64,
    pub(super) blocked: i64,
}

pub(super) async fn count_remaining(
    pool: &TrackedPool<sqlx::Postgres>,
    table: &str,
    cutoff: DateTime<Utc>,
) -> Result<RemainingCandidates, RetentionPurgeError> {
    let statement = remaining_statement(table).ok_or(RetentionPurgeError::TableNotPurgeable)?;
    let row = sqlx::query(sqlx::AssertSqlSafe(statement))
        .bind(cutoff)
        .fetch_one(pool)
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "count_remaining", source })?;
    Ok(RemainingCandidates {
        remaining: row
            .try_get("remaining_rows")
            .map_err(|source| RetentionPurgeError::Query { stage: "count_remaining", source })?,
        blocked: row
            .try_get("blocked_rows")
            .map_err(|source| RetentionPurgeError::Query { stage: "count_remaining", source })?,
    })
}
