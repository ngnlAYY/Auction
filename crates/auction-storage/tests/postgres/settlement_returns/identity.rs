//! 结算退件 Take 一律以冻结的数据库凭据身份执行（PostgreSQL 18，运行账号）：
//! - 本进程不能以持久凭据身份执行 Take 的世界（只配置 Cookie 或未启用邮件），已提交的领回不激活、
//!   不写入投递，案件停在待领回；能冻结身份的世界照常激活；
//! - 已激活但仍待领取的 Take 投递在这些世界不被投递 Worker 领取，同世界的其他投递不受影响；
//! - 详情按在线 Worker 心跳声明的 `settlement_return_take_worlds` 说明“领回能力不可用”，Take 一旦
//!   开始执行就不再显示；说明只读，不改变案件。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration as StdDuration;

use auction_application::delivery::{DeliveryClaimExclusions, DeliveryWorkStore};
use auction_application::settlement::{SettlementReturnAdminPort, SettlementReturnWorkStore};
use auction_domain::{SettlementReturnAction, SettlementReturnCaseStatus};
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use serde_json::json;
use uuid::Uuid;

use super::flows::{handle, returned_payout, seed_spent_payout};
use super::recheck::worker_heartbeat;
use crate::support;

async fn take_unavailable(store: &DatabaseStore, case: Uuid) -> bool {
    store.settlement_return_detail(case).await.expect("案件详情").take_unavailable
}

async fn action_state(store: &DatabaseStore, case: Uuid) -> (String, Option<Uuid>) {
    sqlx::query_as(
        "SELECT execution_state, delivery_id FROM public.settlement_return_actions
         WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取 Take 动作")
}

/// 领取一轮到期投递，返回领取到的投递 ID。
async fn claim_round(store: &DatabaseStore, exclusions: DeliveryClaimExclusions) -> Vec<Uuid> {
    let now = Timestamp::now();
    let grace = StdDuration::from_secs(60);
    DeliveryWorkStore::begin_claim_cycle(store, now, grace).await.expect("领取前清理");
    DeliveryWorkStore::claim_due(
        store,
        "returns-identity-delivery".to_owned(),
        now,
        now + Duration::minutes(2),
        grace,
        exclusions,
        10,
    )
    .await
    .expect("领取投递")
    .into_iter()
    .map(|claim| claim.delivery_id)
    .collect()
}

/// 没有能冻结凭据身份的世界时领回不激活、详情说明领回能力不可用；声明能力的心跳出现后说明消失，
/// 能冻结身份的世界照常激活；激活后的 Take 投递在不能冻结身份的世界不被领取，其他投递不受影响。
#[tokio::test]
async fn postgres_return_take_runs_only_with_a_frozen_credential_identity() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let returned = returned_payout(&store, 7_630_001, "996301").await;
    seed_spent_payout(pool, &returned).await;
    assert_eq!(
        handle(&store, returned.case, 1, SettlementReturnAction::ReclaimAndHold).await,
        SettlementReturnCaseStatus::TakePending
    );

    // 没有世界能冻结凭据身份，或只有其他世界能：不激活、不写入投递。
    for worlds in [Vec::new(), vec!["isekai".to_owned()]] {
        assert!(
            store
                .activate_due_return_take(Timestamp::now(), worlds.clone())
                .await
                .expect("激活")
                .is_none(),
            "{worlds:?}"
        );
    }
    assert_eq!(action_state(&store, returned.case).await, ("pending".to_owned(), None));
    let invalid =
        store.activate_due_return_take(Timestamp::now(), vec!["elsewhere".to_owned()]).await;
    assert!(invalid.is_err(), "世界参数越出闭集时失败关闭");

    // 详情：没有在线 Worker、或在线 Worker 只声明核验能力，都说明领回能力不可用；不改变案件。
    let version: i64 = sqlx::query_scalar(
        "SELECT case_version FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(returned.case)
    .fetch_one(pool)
    .await
    .expect("读取案件版本");
    assert!(take_unavailable(&store, returned.case).await, "没有在线 Worker");
    worker_heartbeat(
        pool,
        "worker-cookie",
        "ready",
        20,
        json!({"settlement_return_recheck_worlds": ["main"], "settlement_return_take_worlds": []}),
    )
    .await;
    assert!(take_unavailable(&store, returned.case).await, "只配置 Cookie 的 Worker");
    worker_heartbeat(
        pool,
        "worker-cookie",
        "ready",
        20,
        json!({"settlement_return_recheck_worlds": ["main"], "settlement_return_take_worlds": ["isekai"]}),
    )
    .await;
    assert!(take_unavailable(&store, returned.case).await, "只声明其他世界");
    worker_heartbeat(
        pool,
        "worker-cookie",
        "ready",
        20,
        json!({"settlement_return_recheck_worlds": ["main"], "settlement_return_take_worlds": ["main"]}),
    )
    .await;
    assert!(!take_unavailable(&store, returned.case).await, "能冻结身份的 Worker 在线");
    let unchanged: i64 = sqlx::query_scalar(
        "SELECT case_version FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(returned.case)
    .fetch_one(pool)
    .await
    .expect("读取案件版本");
    assert_eq!(unchanged, version, "说明只读");

    // 能冻结身份的世界：照常激活为待领取的 Take 投递。
    let progress = store
        .activate_due_return_take(Timestamp::now(), vec!["main".to_owned()])
        .await
        .expect("激活")
        .expect("必须激活");
    assert_eq!(progress.event_kind, "take_activated");
    let (state, take) = action_state(&store, returned.case).await;
    assert_eq!(state, "enqueued");
    let take = take.expect("激活后有 Take 投递");

    // 投递已激活、仍待领取：领回能力消失时仍说明不可用。
    worker_heartbeat(
        pool,
        "worker-cookie",
        "ready",
        20,
        json!({"settlement_return_recheck_worlds": ["main"], "settlement_return_take_worlds": []}),
    )
    .await;
    assert!(take_unavailable(&store, returned.case).await, "已激活但仍待领取");

    // 同世界的其他投递（人工通知）不受影响；结算退件 Take 在不能冻结身份的世界不被领取。
    let other = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, decode(repeat('6d', 32), 'hex'), 'manual', $3, 'SellerPlayer',
                 jsonb_build_object('schema_version', 1, 'operation', 'send', 'world', 'main',
                     'external_mail_id', $1::text, 'template', 'manual_notice',
                     'result_code', 'test', 'user_key_user_id', NULL, 'credits', 0, 'cod', 0,
                     'attachment_ids', jsonb_build_array(), 'body', 'Frozen body'),
                 'pending')",
    )
    .bind(other)
    .bind(Uuid::new_v4())
    .bind(format!("identity:{other}"))
    .execute(pool)
    .await
    .expect("写入同世界的其他投递");
    let claimed = claim_round(
        &store,
        DeliveryClaimExclusions {
            unidentified_take_worlds: vec!["main".to_owned()],
            ..DeliveryClaimExclusions::default()
        },
    )
    .await;
    assert!(claimed.contains(&other), "同世界其他投递照常领取");
    assert!(!claimed.contains(&take), "不能冻结身份的世界不领取结算退件 Take");
    let status: String = sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(take)
        .fetch_one(pool)
        .await
        .expect("读取 Take 投递");
    assert_eq!(status, "pending", "未领取的 Take 留在待执行");
    let invalid = DeliveryWorkStore::claim_due(
        &store,
        "returns-identity-delivery".to_owned(),
        Timestamp::now(),
        Timestamp::now() + Duration::minutes(2),
        StdDuration::from_secs(60),
        DeliveryClaimExclusions {
            unidentified_take_worlds: vec!["elsewhere".to_owned()],
            ..DeliveryClaimExclusions::default()
        },
        10,
    )
    .await;
    assert!(invalid.is_err(), "排除世界越出闭集时失败关闭");

    // 能冻结身份时照常领取；开始执行后不再显示领回能力不可用。
    let claimed = claim_round(&store, DeliveryClaimExclusions::default()).await;
    assert!(claimed.contains(&take), "能冻结身份时照常领取");
    assert!(!take_unavailable(&store, returned.case).await, "Take 已开始执行");

    pool.close().await;
}
