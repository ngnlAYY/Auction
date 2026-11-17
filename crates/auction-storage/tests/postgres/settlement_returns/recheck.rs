//! 待重新核验案件的“核验能力不可用”说明（PostgreSQL 18，运行账号）：只有在线 Worker 的心跳详情
//! 声明能读取案件所在世界的 MoogleMail 时才算可用；没有 Worker、只声明其他世界、心跳过期或正在
//! 停止都显示不可用。说明只读、不改变案件，其他状态恒为不可用说明为假。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{
    SettlementReturnAdminPort, SettlementReturnCommand, SettlementReturnRecheckOutcome,
    SettlementReturnWorkStore,
};
use auction_domain::{CommandId, SettlementReturnAction, SettlementReturnCaseStatus};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{evidence, insert_inbound};
use crate::support;

/// 写入一条 Worker 心跳；`ttl` 为负时已过期。
async fn worker_heartbeat(pool: &PgPool, instance: &str, status: &str, ttl: i64, details: Value) {
    sqlx::query(
        "INSERT INTO public.service_heartbeats
            (service_name, process_role, instance_id, version, started_at, status, readiness,
             audit_failure_count, observation_sequence, observed_at, expires_at, details)
         VALUES ('auction-worker', 'worker', $1, '0.1.5', now() - interval '1 hour', $2, true,
                 0, 1, now() - interval '30 minutes', now() + make_interval(secs => $3), $4)
         ON CONFLICT (service_name, instance_id) DO UPDATE
         SET status = EXCLUDED.status, expires_at = EXCLUDED.expires_at,
             details = EXCLUDED.details",
    )
    .bind(instance)
    .bind(status)
    .bind(ttl as f64)
    .bind(details)
    .execute(pool)
    .await
    .expect("写入 Worker 心跳");
}

async fn unavailable(store: &auction_storage::DatabaseStore, case: Uuid) -> bool {
    store.settlement_return_detail(case).await.expect("案件详情").recheck_unavailable
}

/// 未绑定案件提交重新核验后，详情按在线 Worker 声明的核验世界说明能力是否可用；能力缺失时案件
/// 保持待核验，能力恢复后照常被领取。
#[tokio::test]
async fn postgres_recheck_pending_detail_reports_unavailable_capability() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let unknown = Uuid::new_v4();
    let case = insert_inbound(
        pool,
        "997001",
        Utc::now() - Duration::minutes(5),
        Some(("MoogleMail", format!("[RTS] Auction buyer refund [{unknown}]"))),
        evidence(unknown, 100, &[]),
    )
    .await;
    store.consume_due_settlement_return(Utc::now()).await.expect("消费").expect("必须消费");
    assert!(!unavailable(&store, case).await, "隔离案件不显示核验能力说明");
    let command = SettlementReturnCommand::new(
        CommandId::from_uuid(Uuid::new_v4()),
        case,
        1,
        SettlementReturnAction::Recheck,
        "admin-returns",
        Utc::now(),
    )
    .expect("构造核验命令");
    let result = store.handle_settlement_return(command).await.expect("提交重新核验");
    assert_eq!(result.status, SettlementReturnCaseStatus::RecheckPending);

    // 没有在线 Worker：核验能力不可用。
    assert!(unavailable(&store, case).await);
    // 只声明其他世界、正在停止、心跳过期或未声明该键的 Worker 都不算可用。
    worker_heartbeat(
        pool,
        "worker-isekai",
        "ready",
        30,
        json!({"settlement_return_recheck_worlds": ["isekai"]}),
    )
    .await;
    worker_heartbeat(
        pool,
        "worker-stopping",
        "stopping",
        30,
        json!({"settlement_return_recheck_worlds": ["main"]}),
    )
    .await;
    worker_heartbeat(
        pool,
        "worker-expired",
        "ready",
        -30,
        json!({"settlement_return_recheck_worlds": ["main"]}),
    )
    .await;
    worker_heartbeat(pool, "worker-old", "ready", 30, json!({"forum_sync_enabled": false})).await;
    assert!(unavailable(&store, case).await);
    // 未启用 MoogleMail 的 Worker 声明空集合。
    worker_heartbeat(
        pool,
        "worker-main",
        "ready",
        30,
        json!({"settlement_return_recheck_worlds": []}),
    )
    .await;
    assert!(unavailable(&store, case).await);
    let version_before: i64 = sqlx::query_scalar(
        "SELECT case_version FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取案件版本");

    // 能力恢复：同一在线 Worker 声明 main。
    worker_heartbeat(
        pool,
        "worker-main",
        "degraded",
        30,
        json!({"settlement_return_recheck_worlds": ["isekai", "main"]}),
    )
    .await;
    assert!(!unavailable(&store, case).await);
    let version_after: i64 = sqlx::query_scalar(
        "SELECT case_version FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取案件版本");
    assert_eq!(version_after, version_before, "能力说明只读，不改变案件");

    // 待核验案件照常被领取并写回；之后不再是待核验，不显示说明。
    let claim = store
        .claim_due_return_recheck(Utc::now(), Utc::now() + Duration::minutes(5))
        .await
        .expect("领取核验")
        .expect("必须领取");
    store
        .record_return_recheck(
            claim,
            SettlementReturnRecheckOutcome::NotFound { observed_at: Utc::now() },
        )
        .await
        .expect("写回核验结果");
    worker_heartbeat(
        pool,
        "worker-main",
        "ready",
        30,
        json!({"settlement_return_recheck_worlds": []}),
    )
    .await;
    assert!(!unavailable(&store, case).await, "回到隔离后不显示核验能力说明");

    pool.close().await;
}
