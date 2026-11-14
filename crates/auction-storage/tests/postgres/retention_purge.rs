//! 运行态数据族保留清理的有界执行（政策 v2）。
//!
//! 在临时 PostgreSQL 18 模板库克隆出的独立数据库中验证：
//!   1. 删除边界与保护行：有效/永久会话、未解决失败、被外键阻断的事件、新鲜记录均保留；
//!   2. 同一演练回执的幂等重跑不再删除任何行，审计仍追加成功事实；
//!   3. 批准上限内的删除与计数漂移产生 `partial_limit_reached`，须重新演练；
//!   4. 批次中途失败时已提交批次保留，回执与审计如实报告失败；
//!   5. 无效令牌、过期回执、retain_forever 族与非闭集表在任何变更前拒绝。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_storage::{
    DatabaseStore, RETENTION_DRY_RUN_FORMAT, RetentionDryRunReport, RetentionPurgeApproval,
    RetentionPurgeError, RetentionPurgeFilesystemResult, RetentionPurgeOutcome,
    RetentionPurgeRequest, RetentionPurgeTableApproval, retention_purge_approval_token,
};
use chrono::{DateTime, TimeDelta, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::support;

const RUNTIME_FAMILIES: [&str; 4] =
    ["service_heartbeat", "realtime_events", "realtime_failures", "session_records"];

async fn insert_heartbeat(store: &DatabaseStore, instance: &str, expires_at: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO public.service_heartbeats
            (service_name, process_role, instance_id, version, started_at, status, readiness,
             audit_failure_count, observation_sequence, observed_at, expires_at, details)
         VALUES ('auction-worker', 'worker', $1, 'test', $2, 'ready', true, 0, 1, $2, $3, '{}')",
    )
    .bind(instance)
    .bind(expires_at - TimeDelta::minutes(1))
    .bind(expires_at)
    .execute(store.postgres_pool())
    .await
    .expect("插入心跳");
}

async fn insert_event(
    store: &DatabaseStore,
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
    .execute(store.postgres_pool())
    .await
    .expect("插入实时事件");
    id
}

async fn insert_failure(
    store: &DatabaseStore,
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
    .execute(store.postgres_pool())
    .await
    .expect("插入推送失败");
}

#[allow(clippy::too_many_arguments)]
async fn insert_public_session(
    store: &DatabaseStore,
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
    .execute(store.postgres_pool())
    .await
    .expect("插入 Public 会话");
}

async fn insert_admin_session(
    store: &DatabaseStore,
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
    .execute(store.postgres_pool())
    .await
    .expect("插入 Admin 会话");
}

async fn database_name(store: &DatabaseStore) -> String {
    sqlx::query_scalar("SELECT current_database()")
        .fetch_one(store.postgres_pool())
        .await
        .expect("读取数据库名")
}

async fn collect_report(
    postgres: &auction_storage::DatabaseStore,
    database: &str,
) -> RetentionDryRunReport {
    postgres.collect_retention_dry_run(database, Uuid::new_v4()).await.expect("只读演练")
}

fn approval(
    report: &RetentionDryRunReport,
    database: &str,
    family_name: &str,
) -> RetentionPurgeApproval {
    let family = report
        .families
        .iter()
        .find(|family| family.dataset_family == family_name)
        .unwrap_or_else(|| panic!("缺少数据族 {family_name}"));
    let tables = family
        .tables
        .iter()
        .map(|table| RetentionPurgeTableApproval {
            table: table.table.clone(),
            rule_ids: table.rule_ids.clone(),
            candidate_rows: table.candidate_rows,
        })
        .collect::<Vec<_>>();
    let binding =
        tables.iter().map(|table| (table.table.clone(), table.candidate_rows)).collect::<Vec<_>>();
    let approval_token = retention_purge_approval_token(
        database,
        report.run_id,
        &report.policy_sha256,
        family_name,
        family.candidate_rows,
        None,
        &binding,
    );
    RetentionPurgeApproval {
        dataset_family: family_name.to_owned(),
        approval_token,
        candidate_rows: family.candidate_rows,
        candidate_bytes: None,
        tables,
    }
}

fn request(
    report: &RetentionDryRunReport,
    database: &str,
    approvals: Vec<RetentionPurgeApproval>,
) -> RetentionPurgeRequest {
    RetentionPurgeRequest {
        database_name: database.to_owned(),
        dry_run_run_id: report.run_id,
        dry_run_collected_at: report.collected_at,
        dry_run_created_at: Utc::now(),
        policy_sha256: report.policy_sha256.clone(),
        batch_size: 2,
        approvals,
        filesystem: None,
    }
}

fn all_runtime_approvals(
    report: &RetentionDryRunReport,
    database: &str,
) -> Vec<RetentionPurgeApproval> {
    RUNTIME_FAMILIES.iter().map(|family| approval(report, database, family)).collect()
}

async fn count(store: &DatabaseStore, statement: &'static str) -> i64 {
    sqlx::query_scalar(statement).fetch_one(store.postgres_pool()).await.expect("统计行数")
}

async fn purge_audits(store: &DatabaseStore) -> Vec<(String, serde_json::Value)> {
    let rows = sqlx::query(
        "SELECT result, metadata FROM public.audit_records
         WHERE action = 'auction.retention.purge'
         ORDER BY occurred_at, id",
    )
    .fetch_all(store.postgres_pool())
    .await
    .expect("读取清理审计");
    rows.into_iter()
        .map(|row| {
            (row.try_get("result").expect("result"), row.try_get("metadata").expect("metadata"))
        })
        .collect()
}

#[tokio::test]
async fn postgres_retention_purge_deletes_only_candidates_and_is_idempotent() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let postgres = &store;
    let pool = store.postgres_pool();

    let now = Utc::now();
    let old = |days: i64| now - TimeDelta::days(days);

    // 心跳：31 天前到期（候选）、1 天前到期（保留）、未来到期（保留）。
    insert_heartbeat(&store, "old", old(31)).await;
    insert_heartbeat(&store, "recent", old(1)).await;
    insert_heartbeat(&store, "future", now + TimeDelta::days(1)).await;

    // 实时事件：两条超过 30 天且被失败/重放引用（阻断），一条可删，一条新鲜。
    sqlx::query("INSERT INTO public.realtime_streams (stream_key) VALUES ('events:main')")
        .execute(pool)
        .await
        .expect("插入流");
    let blocked_by_failure = insert_event(&store, "events:main", 1, old(31), None).await;
    let blocked_by_replay = insert_event(&store, "events:main", 2, old(31), None).await;
    let deletable = insert_event(&store, "events:main", 3, old(31), None).await;
    let fresh = insert_event(&store, "events:main", 4, now, Some(blocked_by_replay)).await;
    insert_failure(&store, "consumer", blocked_by_failure, old(31), Some(old(31))).await;
    insert_failure(&store, "consumer-fresh", fresh, old(1), None).await;
    insert_failure(&store, "consumer-resolved", fresh, old(91), Some(old(91))).await;
    insert_failure(&store, "consumer-recent", fresh, old(1), Some(old(1))).await;

    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(user)
        .bind(format!("purge-{}", user.simple()))
        .bind(vec![7_u8; 32])
        .execute(store.postgres_pool())
        .await
        .expect("插入用户");
    insert_public_session(&store, user, 1, old(60), None, Some(old(1))).await;
    insert_public_session(&store, user, 2, old(60), Some(old(31)), None).await;
    insert_public_session(&store, user, 3, old(60), Some(now + TimeDelta::days(30)), None).await;
    insert_public_session(&store, user, 4, old(60), None, None).await;
    insert_public_session(&store, user, 5, old(60), Some(old(1)), None).await;
    insert_admin_session(&store, 10, "revoked", old(60), now + TimeDelta::days(1), Some(old(1)))
        .await;
    insert_admin_session(&store, 11, "active", old(60), now + TimeDelta::days(1), None).await;
    insert_admin_session(&store, 12, "expired", old(60), old(31), None).await;
    insert_admin_session(&store, 13, "grace", old(60), old(1), None).await;

    let database_name = database_name(&store).await;
    let report = collect_report(postgres, &database_name).await;
    assert_eq!(report.format, RETENTION_DRY_RUN_FORMAT);
    let approvals = all_runtime_approvals(&report, &database_name);
    let purge = postgres
        .execute_retention_purge(request(&report, &database_name, approvals))
        .await
        .expect("执行保留清理");

    assert_eq!(purge.outcome, RetentionPurgeOutcome::Complete, "failure={:?}", purge.failure_code);
    assert_eq!(purge.failure_code, None);
    assert!(purge.deletion_authorized);
    assert!(purge.mutation_executed);
    assert_eq!(purge.deleted_rows, 7, "心跳 1 + 事件 1 + 失败 1 + 会话 4");
    assert!(purge.deleted_row_bytes > 0);
    assert!(purge.audit_id.is_some());

    // 心跳：只有过期 31 天的被删除。
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.service_heartbeats").await, 2);
    // 事件：可删事件消失，被引用事件与新鲜事件保留。
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.realtime_events").await, 3);
    let event_remaining = purge
        .families
        .iter()
        .find(|family| family.dataset_family == "realtime_events")
        .expect("事件族");
    assert_eq!(event_remaining.tables[0].blocked_candidate_rows, 2);
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM public.realtime_events WHERE event_id = $1)"
        )
        .bind(deletable)
        .fetch_one(store.postgres_pool())
        .await
        .expect("查询事件"),
        "无引用候选事件必须删除"
    );
    // 失败：只有 90 天前已解决的一条被删除，未解决的仍在。
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.realtime_failures").await, 3);
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*)::bigint FROM public.realtime_failures WHERE resolved_at IS NULL"
        )
        .await,
        1
    );
    // 会话：已注销 + 超宽限过期被删除；有效、永久、宽限内保留。
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.public_sessions").await, 3);
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.admin_sessions").await, 2);
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*)::bigint FROM public.public_sessions
             WHERE revoked_at IS NULL AND expires_at IS NULL"
        )
        .await,
        1,
        "永久 Public 会话必须保留"
    );

    let audits = purge_audits(&store).await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].0, "success");
    assert_eq!(audits[0].1["outcome"], "complete");
    assert_eq!(audits[0].1["deleted_rows"], 7);

    // 同一批准回执幂等重跑：0 行删除、仍追加一条成功审计。
    let rerun = postgres
        .execute_retention_purge(request(
            &report,
            &database_name,
            all_runtime_approvals(&report, &database_name),
        ))
        .await
        .expect("幂等重跑");
    assert_eq!(rerun.outcome, RetentionPurgeOutcome::Complete);
    assert_eq!(rerun.deleted_rows, 0);
    assert_eq!(rerun.deleted_row_bytes, 0);
    assert!(!rerun.mutation_executed);
    assert!(rerun.audit_id.is_some());
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.service_heartbeats").await, 2);
    assert_eq!(purge_audits(&store).await.len(), 2);
    let audits = purge_audits(&store).await;
    assert_eq!(audits[1].1["deleted_rows"], 0);
}

#[tokio::test]
async fn postgres_retention_purge_caps_deletes_at_the_approved_count() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let postgres = &store;
    let now = Utc::now();

    insert_heartbeat(&store, "first", now - TimeDelta::days(31)).await;
    let database_name = database_name(&store).await;
    let report = collect_report(postgres, &database_name).await;
    let heartbeat = heartbeat_family(&report);
    assert_eq!(heartbeat.candidate_rows, 1);

    // 演练之后又有两条过期心跳：批准上限仍绑定演练时的 1 行。
    insert_heartbeat(&store, "second", now - TimeDelta::days(40)).await;
    insert_heartbeat(&store, "third", now - TimeDelta::days(50)).await;

    let purge = postgres
        .execute_retention_purge(request(
            &report,
            &database_name,
            vec![approval(&report, &database_name, "service_heartbeat")],
        ))
        .await
        .expect("执行保留清理");
    assert_eq!(purge.outcome, RetentionPurgeOutcome::PartialLimitReached);
    assert_eq!(purge.deleted_rows, 1);
    let table = &purge.families[0].tables[0];
    assert_eq!(table.approved_rows, 1);
    assert_eq!(table.deleted_rows, 1);
    assert_eq!(table.remaining_candidate_rows, 2, "漂移候选必须留待重新演练");
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.service_heartbeats").await, 2);
    let audit = purge_audits(&store).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].0, "failure", "部分完成如实记录为失败结果");

    // 新演练与新批准删除剩余两条。
    let refreshed = collect_report(postgres, &database_name).await;
    assert_eq!(heartbeat_family(&refreshed).candidate_rows, 2);
    let second = postgres
        .execute_retention_purge(request(
            &refreshed,
            &database_name,
            vec![approval(&refreshed, &database_name, "service_heartbeat")],
        ))
        .await
        .expect("重新演练后执行");
    assert_eq!(second.outcome, RetentionPurgeOutcome::Complete);
    assert_eq!(second.deleted_rows, 2);
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.service_heartbeats").await, 0);
}

#[tokio::test]
async fn postgres_retention_purge_keeps_committed_batches_when_a_later_batch_fails() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let postgres = &store;
    let now = Utc::now();

    insert_heartbeat(&store, "a-oldest", now - TimeDelta::days(50)).await;
    insert_heartbeat(&store, "poison", now - TimeDelta::days(40)).await;
    insert_heartbeat(&store, "c-newest", now - TimeDelta::days(30)).await;
    // DDL 夹具须由维护账号执行；被测清理仍经运行账号。
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");
    sqlx::query(
        "CREATE FUNCTION public.test_purge_poison() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             IF OLD.instance_id = 'poison' THEN
                 RAISE EXCEPTION 'poison';
             END IF;
             RETURN OLD;
         END $$",
    )
    .execute(maintenance.postgres_pool())
    .await
    .expect("创建失败触发器");
    sqlx::query(
        "CREATE TRIGGER test_purge_poison BEFORE DELETE ON public.service_heartbeats
         FOR EACH ROW EXECUTE FUNCTION public.test_purge_poison()",
    )
    .execute(maintenance.postgres_pool())
    .await
    .expect("挂载失败触发器");
    maintenance.postgres_pool().close().await;

    let database_name = database_name(&store).await;
    let report = collect_report(postgres, &database_name).await;
    let mut purge_request = request(
        &report,
        &database_name,
        vec![approval(&report, &database_name, "service_heartbeat")],
    );
    purge_request.batch_size = 1;
    let purge = postgres.execute_retention_purge(purge_request).await.expect("执行保留清理");
    assert_eq!(purge.outcome, RetentionPurgeOutcome::Failed);
    assert_eq!(purge.failure_code, Some("delete_batch_failed"));
    assert_eq!(purge.deleted_rows, 1, "第一批发已提交");
    assert_eq!(purge.families[0].tables[0].batches, 1);
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.service_heartbeats").await, 2);
    let audit = purge_audits(&store).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].0, "failure");
    assert_eq!(audit[0].1["failure_code"], "delete_batch_failed");

    // 故障注入只用于本测试；移除（DDL，维护账号）后剩余两行仍可按新批准删除。
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");
    sqlx::query("DROP TRIGGER test_purge_poison ON public.service_heartbeats")
        .execute(maintenance.postgres_pool())
        .await
        .expect("移除触发器");
    sqlx::query("DROP FUNCTION public.test_purge_poison()")
        .execute(maintenance.postgres_pool())
        .await
        .expect("移除函数");
    maintenance.postgres_pool().close().await;
    let refreshed = collect_report(postgres, &database_name).await;
    let second = postgres
        .execute_retention_purge(request(
            &refreshed,
            &database_name,
            vec![approval(&refreshed, &database_name, "service_heartbeat")],
        ))
        .await
        .expect("修复后执行");
    assert_eq!(second.outcome, RetentionPurgeOutcome::Complete);
    assert_eq!(second.deleted_rows, 2);
}

#[tokio::test]
async fn postgres_retention_purge_rejects_invalid_approvals_before_mutation() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let postgres = &store;
    let now = Utc::now();
    insert_heartbeat(&store, "old", now - TimeDelta::days(31)).await;

    let database_name = database_name(&store).await;
    let report = collect_report(postgres, &database_name).await;
    let valid = approval(&report, &database_name, "service_heartbeat");

    // 错误令牌。
    let mut wrong = valid.clone();
    wrong.approval_token = "0".repeat(64);
    assert!(matches!(
        postgres.execute_retention_purge(request(&report, &database_name, vec![wrong])).await,
        Err(RetentionPurgeError::ApprovalTokenMismatch)
    ));

    // 过期回执。
    let mut stale = request(&report, &database_name, vec![valid.clone()]);
    stale.dry_run_created_at = now - TimeDelta::seconds(901);
    assert!(matches!(
        postgres.execute_retention_purge(stale).await,
        Err(RetentionPurgeError::ReceiptStale)
    ));

    // 回执时间与快照时间不配套。
    let mut incoherent = request(&report, &database_name, vec![valid.clone()]);
    incoherent.dry_run_collected_at = incoherent.dry_run_created_at - TimeDelta::seconds(700);
    assert!(matches!(
        postgres.execute_retention_purge(incoherent).await,
        Err(RetentionPurgeError::InvalidRequest)
    ));

    // retain_forever 族：审计族永不清理。
    let audit_family = RetentionPurgeApproval {
        dataset_family: "audit".to_owned(),
        approval_token: "0".repeat(64),
        candidate_rows: 0,
        candidate_bytes: None,
        tables: Vec::new(),
    };
    assert!(matches!(
        postgres
            .execute_retention_purge(request(&report, &database_name, vec![audit_family]))
            .await,
        Err(RetentionPurgeError::RetainForeverFamily)
    ));

    // 非闭集表：即使伪造令牌也在校验阶段拒绝。
    let mut substituted = valid.clone();
    substituted.tables = vec![RetentionPurgeTableApproval {
        table: "audit_records".to_owned(),
        rule_ids: vec!["service_heartbeat_expired_grace".to_owned()],
        candidate_rows: 1,
    }];
    assert!(matches!(
        postgres.execute_retention_purge(request(&report, &database_name, vec![substituted])).await,
        Err(RetentionPurgeError::ApprovalInvalid | RetentionPurgeError::TableNotPurgeable)
    ));

    // 全部拒绝路径都不改变数据、不追加审计。
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.service_heartbeats").await, 1);
    assert!(purge_audits(&store).await.is_empty());

    // 有效批准仍可执行。
    let purge = postgres
        .execute_retention_purge(request(&report, &database_name, vec![valid]))
        .await
        .expect("有效批准执行");
    assert_eq!(purge.outcome, RetentionPurgeOutcome::Complete);
    assert_eq!(purge.deleted_rows, 1);
    assert_eq!(purge_audits(&store).await.len(), 1);
}

#[tokio::test]
async fn postgres_retention_purge_unblocks_referenced_events_in_the_same_run() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let postgres = &store;
    let now = Utc::now();

    sqlx::query("INSERT INTO public.realtime_streams (stream_key) VALUES ('events:main')")
        .execute(store.postgres_pool())
        .await
        .expect("插入流");
    let event = insert_event(&store, "events:main", 1, now - TimeDelta::days(100), None).await;
    insert_failure(
        &store,
        "unblocking",
        event,
        now - TimeDelta::days(100),
        Some(now - TimeDelta::days(91)),
    )
    .await;

    let database_name = database_name(&store).await;
    let report = collect_report(postgres, &database_name).await;
    let approvals = vec![
        approval(&report, &database_name, "realtime_events"),
        approval(&report, &database_name, "realtime_failures"),
    ];
    let purge = postgres
        .execute_retention_purge(request(&report, &database_name, approvals))
        .await
        .expect("执行保留清理");
    assert_eq!(purge.outcome, RetentionPurgeOutcome::Complete);
    // 先删已解决的失败，再删因此解除 RESTRICT 引用的事件；两族都在同一轮完成。
    assert_eq!(purge.deleted_rows, 2);
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.realtime_events").await, 0);
    assert_eq!(count(&store, "SELECT COUNT(*)::bigint FROM public.realtime_failures").await, 0);
    assert_eq!(
        purge
            .families
            .iter()
            .find(|family| family.dataset_family == "realtime_events")
            .expect("事件族")
            .tables[0]
            .remaining_candidate_rows,
        0
    );

    // 同一批准再跑：事件与失败均无候选，零删除。
    let rerun = postgres
        .execute_retention_purge(request(
            &report,
            &database_name,
            vec![
                approval(&report, &database_name, "realtime_events"),
                approval(&report, &database_name, "realtime_failures"),
            ],
        ))
        .await
        .expect("幂等重跑");
    assert_eq!(rerun.outcome, RetentionPurgeOutcome::Complete);
    assert_eq!(rerun.deleted_rows, 0);
    assert!(!rerun.mutation_executed);
}

#[tokio::test]
async fn postgres_retention_purge_accepts_mixed_database_and_filesystem_approvals() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let postgres = &store;
    let database_name = database_name(&store).await;
    let report = collect_report(postgres, &database_name).await;

    // 文件族由 CLI 在预校验后执行；这里只验证存储层接受混合批准并如实计入回执。
    let filesystem_token = retention_purge_approval_token(
        &database_name,
        report.run_id,
        &report.policy_sha256,
        "mail_diagnostic_files",
        1,
        Some(75),
        &[],
    );
    let filesystem_approval = RetentionPurgeApproval {
        dataset_family: "mail_diagnostic_files".to_owned(),
        approval_token: filesystem_token,
        candidate_rows: 1,
        candidate_bytes: Some(75),
        tables: Vec::new(),
    };
    let mut purge_request = request(
        &report,
        &database_name,
        vec![approval(&report, &database_name, "service_heartbeat"), filesystem_approval],
    );
    purge_request.filesystem = Some(RetentionPurgeFilesystemResult {
        directory_present: true,
        examined_files: 4,
        deleted_files: 1,
        deleted_bytes: 75,
        retained_files: 2,
        write_unknown_held_files: 1,
        write_unknown_over_hold_window_files: 1,
        unexpected_entries: 0,
        limit_reached: false,
        needs_follow_up: true,
        failure_code: None,
    });
    let purge = postgres.execute_retention_purge(purge_request).await.expect("混合批准执行");
    assert_eq!(purge.outcome, RetentionPurgeOutcome::Complete);
    assert_eq!(purge.deleted_rows, 0);
    assert!(purge.mutation_executed, "文件族已实际删除 1 个文件");
    assert_eq!(purge.filesystem.as_ref().expect("文件结果").deleted_files, 1);
    assert_eq!(purge.deleted_row_bytes, 0);
    let audit = purge_audits(&store).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].1["mutation_executed"], true);
    assert_eq!(audit[0].1["filesystem"]["deleted_files"], 1);
}

fn heartbeat_family(report: &RetentionDryRunReport) -> &auction_storage::RetentionFamilyDryRun {
    report
        .families
        .iter()
        .find(|family| family.dataset_family == "service_heartbeat")
        .expect("心跳族")
}
