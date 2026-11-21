//! PostgreSQL 集成测试中按指定阻塞事务统计锁等待者。

use sqlx::PgPool;

/// 统计本测试库中沿阻塞链最终被 `blocker_pid` 阻塞的锁等待会话数。
pub async fn count_blocked_by(pool: &PgPool, blocker_pid: i32) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "WITH RECURSIVE seeded_blocker(pid) AS (
             SELECT pid
             FROM pg_stat_activity
             WHERE pid = $1 AND datname = current_database()
         ),
         blocker_chain(waiter_pid, blocker_pid, path) AS (
             SELECT activity.pid,
                    direct.blocker_pid,
                    ARRAY[activity.pid, direct.blocker_pid]::integer[]
             FROM pg_stat_activity AS activity
             CROSS JOIN LATERAL unnest(pg_blocking_pids(activity.pid)) AS direct(blocker_pid)
             WHERE activity.datname = current_database()
               AND activity.wait_event_type = 'Lock'
               AND activity.pid <> $1
             UNION ALL
             SELECT chain.waiter_pid,
                    next_blocker.blocker_pid,
                    chain.path || next_blocker.blocker_pid
             FROM blocker_chain AS chain
             CROSS JOIN LATERAL unnest(pg_blocking_pids(chain.blocker_pid))
                 AS next_blocker(blocker_pid)
             WHERE NOT next_blocker.blocker_pid = ANY(chain.path)
         )
         SELECT count(DISTINCT waiter_pid)
         FROM blocker_chain
         WHERE EXISTS (
             SELECT 1
             FROM seeded_blocker
             WHERE seeded_blocker.pid = blocker_chain.blocker_pid
         )",
    )
    .bind(blocker_pid)
    .fetch_one(pool)
    .await
}
