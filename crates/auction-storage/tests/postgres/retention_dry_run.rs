//! 运行态数据族保留清理的只读演练（政策 v2）。
//!
//! 在临时 PostgreSQL 18 模板库（全部内嵌迁移已执行）插入边界夹具，验证：
//!   1. 进程心跳、realtime 事件/已解决失败、会话的候选计数与边界；
//!   2. 有效（含永久）会话被排除，已注销会话成为候选，宽限期内会话不是候选；
//!   3. 被失败记录或重放关系引用的事件计入 `blocked_rows`，不被假装可删；
//!   4. 报告恒为 `mutation_executed=false`、`deletion_authorized=false`，且确实运行在只读事务。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::support;

use auction_storage::{RETENTION_DRY_RUN_FORMAT, RetentionDryRunReport, RetentionTableDryRun};
use chrono::{DateTime, TimeDelta, Utc};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

async fn insert_heartbeat(
    connection: &mut PgConnection,
    instance: &str,
    expires_at: DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO public.service_heartbeats
            (service_name, process_role, instance_id, version, started_at, status, readiness,
             audit_failure_count, observation_sequence, observed_at, expires_at, details)
         VALUES ('auction-worker', 'worker', $1, 'test', $2, 'ready', true, 0, 1, $2, $3, '{}')",
    )
    .bind(instance)
    .bind(expires_at - TimeDelta::minutes(1))
    .bind(expires_at)
    .execute(&mut *connection)
    .await
    .expect("插入心跳");
}

async fn insert_event(
    connection: &mut PgConnection,
    stream: &str,
    sequence: i64,
    created_at: DateTime<Utc>,
    replay_of: Option<Uuid>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             created_at, replay_of_event_id)
         VALUES ($1, $2, $3, 'auction.list_changed', 1, '{}', $4, $5)",
    )
    .bind(id)
    .bind(stream)
    .bind(sequence)
    .bind(created_at)
    .bind(replay_of)
    .execute(&mut *connection)
    .await
    .expect("插入实时事件");
    id
}

async fn insert_failure(
    connection: &mut PgConnection,
    consumer: &str,
    event_id: Uuid,
    failed_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
) {
    sqlx::query(
        "INSERT INTO public.realtime_failures
            (id, consumer_key, event_id, failure_code, safe_detail, failed_at, resolved_at)
         VALUES ($1, $2, $3, 'delivery_failed', 'test', $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(consumer)
    .bind(event_id)
    .bind(failed_at)
    .bind(resolved_at)
    .execute(&mut *connection)
    .await
    .expect("插入推送失败");
}

#[allow(clippy::too_many_arguments)]
async fn insert_public_session(
    connection: &mut PgConnection,
    user_id: Uuid,
    token: u8,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
) {
    sqlx::query(
        "INSERT INTO public.public_sessions
            (token_hash, user_id, created_at, last_activity_at, expires_at, absolute_expires_at,
             revoked_at)
         VALUES ($1, $2, $3, $3, $4, $4, $5)",
    )
    .bind(vec![token; 32])
    .bind(user_id)
    .bind(created_at)
    .bind(expires_at)
    .bind(revoked_at)
    .execute(&mut *connection)
    .await
    .expect("插入 Public 会话");
}

async fn insert_admin_session(
    connection: &mut PgConnection,
    token: u8,
    actor: &str,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
) {
    sqlx::query(
        "INSERT INTO public.admin_sessions
            (token_hash, actor, csrf_hash, created_at, last_activity_at, expires_at,
             absolute_expires_at, revoked_at)
         VALUES ($1, $6, $2, $3, $3, $4, $4, $5)",
    )
    .bind(vec![token; 32])
    .bind(vec![token.wrapping_add(1); 32])
    .bind(created_at)
    .bind(expires_at)
    .bind(revoked_at)
    .bind(actor)
    .execute(&mut *connection)
    .await
    .expect("插入 Admin 会话");
}

fn table<'a>(
    report: &'a RetentionDryRunReport,
    family: &str,
    name: &str,
) -> &'a RetentionTableDryRun {
    report
        .families
        .iter()
        .find(|candidate| candidate.dataset_family == family)
        .unwrap_or_else(|| panic!("缺少数据族 {family}"))
        .tables
        .iter()
        .find(|candidate| candidate.table == name)
        .unwrap_or_else(|| panic!("缺少表 {name}"))
}

#[tokio::test]
async fn postgres_retention_dry_run_counts_runtime_candidates_without_mutation() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let postgres = &store;
    let pool = store.postgres_pool();
    let mut connection = pool.acquire().await.expect("获取测试连接");

    let now = Utc::now();
    let old = |days: i64| now - TimeDelta::days(days);

    // 心跳：31 天前到期（候选）、1 天前到期（宽限内保留）、未来到期（保留）。
    insert_heartbeat(&mut connection, "old", old(31)).await;
    insert_heartbeat(&mut connection, "recent", old(1)).await;
    insert_heartbeat(&mut connection, "future", now + TimeDelta::days(1)).await;

    // 实时事件：三条超过 30 天。第一条被失败记录引用、第二条被新事件重放引用，
    // 第三条无引用；新事件排除。
    sqlx::query("INSERT INTO public.realtime_streams (stream_key) VALUES ('events:main')")
        .execute(&mut *connection)
        .await
        .expect("插入流");
    let blocked_by_failure = insert_event(&mut connection, "events:main", 1, old(31), None).await;
    let blocked_by_replay = insert_event(&mut connection, "events:main", 2, old(31), None).await;
    let deletable = insert_event(&mut connection, "events:main", 3, old(31), None).await;
    let fresh = insert_event(&mut connection, "events:main", 4, now, Some(blocked_by_replay)).await;
    insert_failure(&mut connection, "consumer", blocked_by_failure, old(31), Some(old(31))).await;
    insert_failure(&mut connection, "consumer-fresh", fresh, old(1), None).await;
    insert_failure(&mut connection, "consumer-resolved", fresh, old(91), Some(old(91))).await;
    insert_failure(&mut connection, "consumer-recent", fresh, old(1), Some(old(1))).await;
    // 唯一约束 (consumer_key, event_id)；三条失败使用不同 consumer，事件引用保持上面两处。

    // 会话：已注销、超宽限过期是候选；有效、永久、宽限内全部排除。
    let user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, 'retention-tester', $2)",
    )
    .bind(user)
    .bind(vec![7_u8; 32])
    .execute(&mut *connection)
    .await
    .expect("插入用户");
    insert_public_session(&mut connection, user, 1, old(60), None, Some(old(1))).await;
    insert_public_session(&mut connection, user, 2, old(60), Some(old(31)), None).await;
    insert_public_session(&mut connection, user, 3, old(60), Some(now + TimeDelta::days(30)), None)
        .await;
    insert_public_session(&mut connection, user, 4, old(60), None, None).await;
    insert_public_session(&mut connection, user, 5, old(60), Some(old(1)), None).await;
    insert_admin_session(
        &mut connection,
        10,
        "revoked",
        old(60),
        now + TimeDelta::days(1),
        Some(old(1)),
    )
    .await;
    insert_admin_session(&mut connection, 11, "active", old(60), now + TimeDelta::days(1), None)
        .await;
    insert_admin_session(&mut connection, 12, "expired", old(60), old(31), None).await;
    insert_admin_session(&mut connection, 13, "grace", old(60), old(1), None).await;

    let database_name: String = sqlx::query("SELECT current_database() AS name")
        .fetch_one(&mut *connection)
        .await
        .expect("读取数据库名")
        .try_get("name")
        .expect("数据库名列");
    drop(connection);

    let run_id = Uuid::new_v4();
    let report =
        postgres.collect_retention_dry_run(&database_name, run_id).await.expect("只读演练");

    assert_eq!(report.format, RETENTION_DRY_RUN_FORMAT);
    assert_eq!(report.run_id, run_id);
    assert_eq!(report.database_name, database_name);
    assert_eq!(report.transaction_read_only, "on");
    assert_eq!(report.policy_version, 2);
    assert!(!report.mutation_executed);
    assert!(!report.deletion_authorized);
    assert!(report.deletion_requires_dry_run_and_operator_approval);

    let heartbeat = table(&report, "service_heartbeat", "service_heartbeats");
    assert_eq!(
        (heartbeat.total_rows, heartbeat.candidate_rows, heartbeat.retained_rows),
        (3, 1, 2)
    );

    let events = table(&report, "realtime_events", "realtime_events");
    assert_eq!(events.total_rows, 4);
    assert_eq!(events.candidate_rows, 3, "只有三条超过 30 天的事件是候选");
    assert_eq!(events.blocked_rows, 2, "被失败记录或重放引用的候选必须计入阻断");
    assert_eq!(events.retained_rows, 1);

    let failures = table(&report, "realtime_failures", "realtime_failures");
    assert_eq!(failures.total_rows, 4);
    assert_eq!(failures.candidate_rows, 1, "只有 90 天前已解决的失败是候选");
    assert_eq!(failures.unresolved_rows, 1);
    assert_eq!(failures.retained_rows, 3);

    let public = table(&report, "session_records", "public_sessions");
    assert_eq!(public.total_rows, 5);
    assert_eq!(public.candidate_rows, 2, "已注销 + 超过 30 天过期");
    assert_eq!(public.excluded_active_rows, 2, "有效与会话永久");
    assert_eq!(public.grace_rows, 1, "过期但在 30 天宽限内");
    assert_eq!(public.candidate_rows + public.retained_rows, public.total_rows);

    let admin = table(&report, "session_records", "admin_sessions");
    assert_eq!(admin.total_rows, 4);
    assert_eq!(admin.candidate_rows, 2);
    assert_eq!(admin.excluded_active_rows, 1);
    assert_eq!(admin.grace_rows, 1);

    let sessions = report
        .families
        .iter()
        .find(|family| family.dataset_family == "session_records")
        .expect("会话族");
    assert_eq!(sessions.candidate_rows, 4);
    assert_eq!(sessions.excluded_active_rows, 3);

    // 受保护族不出现清理规则，也不会被演练计数。
    for name in ["audit", "auction_history", "settlement_compensation_recovery"] {
        assert!(report.families.iter().all(|family| family.dataset_family != name));
    }

    // 演练后行数不变：没有任何 DELETE。
    let remaining: i64 = sqlx::query(
        "SELECT (SELECT COUNT(*) FROM public.service_heartbeats)
              + (SELECT COUNT(*) FROM public.realtime_events)
              + (SELECT COUNT(*) FROM public.realtime_failures)
              + (SELECT COUNT(*) FROM public.public_sessions)
              + (SELECT COUNT(*) FROM public.admin_sessions) AS total",
    )
    .fetch_one(pool)
    .await
    .expect("统计行数")
    .try_get("total")
    .expect("total 列");
    assert_eq!(remaining, 3 + 4 + 4 + 5 + 4);

    // 清理候选涉及的事件与失败事实确实仍可被读取；`deletable` 未被误删。
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.realtime_events WHERE event_id = $1)",
    )
    .bind(deletable)
    .fetch_one(pool)
    .await
    .expect("读取事件");
    assert!(exists);
}
