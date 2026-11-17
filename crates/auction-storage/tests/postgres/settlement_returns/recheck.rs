//! 待重新核验案件的“核验能力不可用”说明（PostgreSQL 18，运行账号）：只有 Worker 的心跳详情声明
//! 能读取案件所在世界的 MoogleMail 时才算可用；没有 Worker、只声明其他世界、心跳超出宽限窗口或
//! 正在停止都显示不可用。最近一次心跳过期后一个 TTL 内（重启空窗）仍算可用。说明只读、不改变
//! 案件，其他状态恒为不可用说明为假。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{
    SettlementReturnAdminPort, SettlementReturnCommand, SettlementReturnRecheckOutcome,
    SettlementReturnWorkStore,
};
use auction_domain::{CommandId, SettlementReturnAction, SettlementReturnCaseStatus};
use auction_time::{Duration, Timestamp};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{evidence, insert_inbound};
use crate::support;

/// 写入一条 Worker 心跳：与 Worker 发布同一 TTL（30 秒），`ttl` 为距过期的剩余秒数，为负时已
/// 过期；观察时刻为过期时刻减 30 秒。
pub(super) async fn worker_heartbeat(
    pool: &PgPool,
    instance: &str,
    status: &str,
    ttl: i64,
    details: Value,
) {
    sqlx::query(
        "INSERT INTO public.service_heartbeats
            (service_name, process_role, instance_id, version, started_at, status, readiness,
             audit_failure_count, observation_sequence, observed_at, expires_at, details)
         VALUES ('auction-worker', 'worker', $1, '0.1.5', now() - interval '1 hour', $2, true,
                 0, 1, clock_timestamp() + make_interval(secs => $3) - interval '30 seconds',
                 clock_timestamp() + make_interval(secs => $3), $4)
         ON CONFLICT (service_name, instance_id) DO UPDATE
         SET status = EXCLUDED.status, observed_at = EXCLUDED.observed_at,
             expires_at = EXCLUDED.expires_at, details = EXCLUDED.details",
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
        Timestamp::now() - Duration::minutes(5),
        Some(("MoogleMail", format!("[RTS] Auction buyer refund [{unknown}]"))),
        evidence(unknown, 100, &[]),
    )
    .await;
    store.consume_due_settlement_return(Timestamp::now()).await.expect("消费").expect("必须消费");
    assert!(!unavailable(&store, case).await, "隔离案件不显示核验能力说明");
    let command = SettlementReturnCommand::new(
        CommandId::from_uuid(Uuid::new_v4()),
        case,
        1,
        SettlementReturnAction::Recheck,
        "admin-returns",
        Timestamp::now(),
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
        -45,
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
        .claim_due_return_recheck(Timestamp::now(), Timestamp::now() + Duration::minutes(5))
        .await
        .expect("领取核验")
        .expect("必须领取");
    store
        .record_return_recheck(
            claim,
            SettlementReturnRecheckOutcome::NotFound { observed_at: Timestamp::now() },
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

/// 建立一件待重新核验的未绑定案件。
async fn recheck_pending_case(store: &auction_storage::DatabaseStore, mid: &str) -> Uuid {
    let unknown = Uuid::new_v4();
    let case = insert_inbound(
        store.postgres_pool(),
        mid,
        Timestamp::now() - Duration::minutes(5),
        Some(("MoogleMail", format!("[RTS] Auction buyer refund [{unknown}]"))),
        evidence(unknown, 100, &[]),
    )
    .await;
    store.consume_due_settlement_return(Timestamp::now()).await.expect("消费").expect("必须消费");
    let command = SettlementReturnCommand::new(
        CommandId::from_uuid(Uuid::new_v4()),
        case,
        1,
        SettlementReturnAction::Recheck,
        "admin-returns",
        Timestamp::now(),
    )
    .expect("构造核验命令");
    let result = store.handle_settlement_return(command).await.expect("提交重新核验");
    assert_eq!(result.status, SettlementReturnCaseStatus::RecheckPending);
    case
}

/// 心跳宽限窗口的边界：最近一次心跳过期后一个 TTL（30 秒）内仍算可用，超出即不可用；最近一次
/// 心跳处于停止中、或最近一次心跳不再声明该世界时，旧心跳即使仍在宽限窗口内也不算可用；有效期
/// 内声明该世界的新实例总是可用。
#[tokio::test]
async fn postgres_recheck_capability_heartbeat_grace_boundaries() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let case = recheck_pending_case(&store, "997101").await;
    let main = || json!({"settlement_return_recheck_worlds": ["main"]});

    // Worker 崩溃后重启的空窗：旧实例最后一次 ready 心跳刚过期 10 秒，新实例尚未发布。
    worker_heartbeat(pool, "worker-crashed", "ready", -10, main()).await;
    assert!(!unavailable(&store, case).await, "过期后一个 TTL 内仍在宽限窗口");
    // 接近窗口末端（过期 25 秒）仍可用；过期 35 秒已超出窗口。
    worker_heartbeat(pool, "worker-crashed", "ready", -25, main()).await;
    assert!(!unavailable(&store, case).await, "宽限窗口末端之前仍可用");
    worker_heartbeat(pool, "worker-crashed", "ready", -35, main()).await;
    assert!(unavailable(&store, case).await, "超出宽限窗口即不可用");

    // 正常停止：最近一次心跳是 stopping，即使仍在有效期内也不可用，较早的旧心跳也不算。
    worker_heartbeat(pool, "worker-crashed", "ready", -10, main()).await;
    worker_heartbeat(pool, "worker-stopped", "stopping", 30, main()).await;
    assert!(unavailable(&store, case).await, "最近一次心跳处于停止中");

    // 新实例发布 starting 并声明该世界：有效期内总是可用。
    worker_heartbeat(pool, "worker-restarted", "starting", 30, main()).await;
    assert!(!unavailable(&store, case).await, "新实例在有效期内声明能力");

    // 新实例的配置不再声明 main：最近一次心跳不声明，宽限窗口内的旧心跳不能顶替。
    worker_heartbeat(pool, "worker-restarted", "ready", -100, main()).await;
    worker_heartbeat(pool, "worker-stopped", "ready", -40, main()).await;
    worker_heartbeat(pool, "worker-crashed", "ready", -5, main()).await;
    assert!(!unavailable(&store, case).await, "旧实例在宽限窗口内");
    worker_heartbeat(
        pool,
        "worker-reconfigured",
        "starting",
        30,
        json!({"settlement_return_recheck_worlds": ["isekai"]}),
    )
    .await;
    assert!(unavailable(&store, case).await, "最近一次心跳不再声明该世界");

    pool.close().await;
}
