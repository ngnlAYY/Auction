//! 单表候选计数；只执行 SELECT，谓词与 `limits` 的边界函数和 SQL 时间戳一一对应。

use chrono::{DateTime, Utc};
use sqlx::Row;

use super::model::RetentionTableDryRun;
use super::{RetentionDryRunError, query_error};

pub(super) struct HeartbeatCounts {
    pub(super) total: i64,
    pub(super) candidates: i64,
}

pub(super) struct RealtimeEventCounts {
    pub(super) total: i64,
    pub(super) candidates: i64,
    pub(super) blocked: i64,
}

pub(super) struct FailureCounts {
    pub(super) total: i64,
    pub(super) candidates: i64,
    pub(super) unresolved: i64,
}

pub(super) struct SessionCounts {
    pub(super) total: i64,
    pub(super) revoked: i64,
    pub(super) expired: i64,
    pub(super) permanent_active: i64,
    pub(super) active: i64,
}

pub(super) async fn heartbeats(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    cutoff: DateTime<Utc>,
) -> Result<HeartbeatCounts, RetentionDryRunError> {
    let row = sqlx::query(
        "SELECT COUNT(*)::bigint AS total_rows,
                COUNT(*) FILTER (WHERE expires_at <= $1)::bigint AS candidate_rows
         FROM public.service_heartbeats",
    )
    .bind(cutoff)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("service_heartbeats", source))?;
    Ok(HeartbeatCounts {
        total: row
            .try_get("total_rows")
            .map_err(|source| query_error("service_heartbeats", source))?,
        candidates: row
            .try_get("candidate_rows")
            .map_err(|source| query_error("service_heartbeats", source))?,
    })
}

/// 事件候选只按年龄；被失败记录（event_id 或 replay_event_id）或重放关系引用的候选行
/// 仍受 `ON DELETE RESTRICT` 外键阻断，单独计数而不假装可删。
pub(super) async fn realtime_events(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    cutoff: DateTime<Utc>,
) -> Result<RealtimeEventCounts, RetentionDryRunError> {
    let row = sqlx::query(
        "SELECT COUNT(*)::bigint AS total_rows,
                COUNT(*) FILTER (WHERE event.created_at <= $1)::bigint AS candidate_rows,
                COUNT(*) FILTER (
                    WHERE event.created_at <= $1
                      AND (
                        EXISTS (
                            SELECT 1 FROM public.realtime_failures AS failure
                            WHERE failure.event_id = event.event_id
                               OR failure.replay_event_id = event.event_id
                        )
                        OR EXISTS (
                            SELECT 1 FROM public.realtime_events AS replay
                            WHERE replay.replay_of_event_id = event.event_id
                        )
                      )
                )::bigint AS blocked_rows
         FROM public.realtime_events AS event",
    )
    .bind(cutoff)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("realtime_events", source))?;
    Ok(RealtimeEventCounts {
        total: row
            .try_get("total_rows")
            .map_err(|source| query_error("realtime_events", source))?,
        candidates: row
            .try_get("candidate_rows")
            .map_err(|source| query_error("realtime_events", source))?,
        blocked: row
            .try_get("blocked_rows")
            .map_err(|source| query_error("realtime_events", source))?,
    })
}

pub(super) async fn realtime_failures(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    cutoff: DateTime<Utc>,
) -> Result<FailureCounts, RetentionDryRunError> {
    let row = sqlx::query(
        "SELECT COUNT(*)::bigint AS total_rows,
                COUNT(*) FILTER (WHERE resolved_at IS NULL)::bigint AS unresolved_rows,
                COUNT(*) FILTER (
                    WHERE resolved_at IS NOT NULL AND resolved_at <= $1
                )::bigint AS candidate_rows
         FROM public.realtime_failures",
    )
    .bind(cutoff)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("realtime_failures", source))?;
    Ok(FailureCounts {
        total: row
            .try_get("total_rows")
            .map_err(|source| query_error("realtime_failures", source))?,
        candidates: row
            .try_get("candidate_rows")
            .map_err(|source| query_error("realtime_failures", source))?,
        unresolved: row
            .try_get("unresolved_rows")
            .map_err(|source| query_error("realtime_failures", source))?,
    })
}

/// 会话三分类：已注销、已过期满 30 天、其余保留。Public 永久会话（两个截止均为 NULL）
/// 只会在已注销时成为候选；`expires_at` 是有效截止（约束保证 absolute >= expires）。
pub(super) async fn sessions(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    table: &'static str,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<SessionCounts, RetentionDryRunError> {
    let statement = format!(
        "SELECT COUNT(*)::bigint AS total_rows,
                COUNT(*) FILTER (WHERE revoked_at IS NOT NULL)::bigint AS revoked_rows,
                COUNT(*) FILTER (
                    WHERE revoked_at IS NULL AND expires_at IS NOT NULL AND expires_at <= $1
                )::bigint AS expired_rows,
                COUNT(*) FILTER (WHERE revoked_at IS NULL AND expires_at IS NULL)::bigint
                    AS permanent_active_rows,
                COUNT(*) FILTER (
                    WHERE revoked_at IS NULL AND expires_at > $2
                )::bigint AS active_rows
         FROM public.{}",
        crate::database::quote_identifier(table)
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(statement))
        .bind(cutoff)
        .bind(now)
        .fetch_one(&mut **transaction)
        .await
        .map_err(|source| query_error("session_records", source))?;
    Ok(SessionCounts {
        total: row
            .try_get("total_rows")
            .map_err(|source| query_error("session_records", source))?,
        revoked: row
            .try_get("revoked_rows")
            .map_err(|source| query_error("session_records", source))?,
        expired: row
            .try_get("expired_rows")
            .map_err(|source| query_error("session_records", source))?,
        permanent_active: row
            .try_get("permanent_active_rows")
            .map_err(|source| query_error("session_records", source))?,
        active: row
            .try_get("active_rows")
            .map_err(|source| query_error("session_records", source))?,
    })
}

pub(super) fn session_table_counts(
    table: &'static str,
    counts: SessionCounts,
    rule_ids: Vec<String>,
) -> Result<RetentionTableDryRun, RetentionDryRunError> {
    let candidates = add(counts.revoked, counts.expired)?;
    let excluded_active = add(counts.active, counts.permanent_active)?;
    let retained = sub(counts.total, candidates)?;
    let grace = sub(retained, excluded_active)?;
    if retained < 0 || grace < 0 {
        return Err(RetentionDryRunError::CountOverflow);
    }
    Ok(RetentionTableDryRun {
        table: table.to_owned(),
        rule_ids,
        total_rows: counts.total,
        candidate_rows: candidates,
        retained_rows: retained,
        blocked_rows: 0,
        excluded_active_rows: excluded_active,
        grace_rows: grace,
        unresolved_rows: 0,
    })
}

pub(super) fn add(left: i64, right: i64) -> Result<i64, RetentionDryRunError> {
    left.checked_add(right).ok_or(RetentionDryRunError::CountOverflow)
}

pub(super) fn sub(left: i64, right: i64) -> Result<i64, RetentionDryRunError> {
    left.checked_sub(right).ok_or(RetentionDryRunError::CountOverflow)
}

#[cfg(test)]
mod tests {
    use super::{SessionCounts, session_table_counts};

    #[test]
    fn session_table_projection_partitions_total() {
        let counts =
            SessionCounts { total: 10, revoked: 2, expired: 3, permanent_active: 1, active: 3 };
        let table = session_table_counts("public_sessions", counts, vec!["rule".to_owned()])
            .expect("projection");
        assert_eq!(table.candidate_rows, 5);
        assert_eq!(table.retained_rows, 5);
        assert_eq!(table.excluded_active_rows, 4);
        assert_eq!(table.grace_rows, 1);
        assert_eq!(
            table.candidate_rows + table.retained_rows,
            table.total_rows,
            "候选与保留必须恰好覆盖总行数"
        );
    }

    #[test]
    fn session_projection_rejects_inconsistent_counts() {
        let counts =
            SessionCounts { total: 1, revoked: 2, expired: 0, permanent_active: 0, active: 0 };
        assert!(session_table_counts("public_sessions", counts, Vec::new()).is_err());
    }
}
