//! 多个用例共用的种子与观测夹具：冻结策略快照、稳定标识、测试用户、场次回收、advisory 锁
//! 计数与条件等待。
//!
//! 冻结策略经生产模板 `AuctionPolicyTemplate::snapshot` 生成，夹具随生产快照形状演进，
//! 不再各自手写可能过时的 JSON。
#![allow(clippy::expect_used)]

use std::future::Future;
use std::time::{Duration, Instant};

use auction_application::auction::AuctionPolicyTemplate;
use auction_storage::DatabaseStore;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// 生产形状的冻结策略快照：0 ppm 的统一最低加价策略，起拍价与最低加价由用例指定。
pub fn policy_snapshot(deadline: DateTime<Utc>, start_price: i64, minimum_increment: i64) -> Value {
    let template = AuctionPolicyTemplate {
        start_price,
        bid_increment_ppm: 0,
        ..AuctionPolicyTemplate::default()
    };
    template.validate().expect("测试策略模板有效");
    let mut snapshot = template.snapshot(deadline);
    snapshot["minimum_increment"] = json!(minimum_increment);
    snapshot
}

/// 由命名空间与后缀派生的稳定 UUID；同一用例重跑得到同一组标识。
pub fn stable_id(namespace: &str, suffix: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{namespace}-{suffix}").as_bytes())
}

/// 写入一名测试用户；公钥摘要由用户名派生，同库内互不相同。
pub async fn seed_user(pool: &PgPool, user_id: Uuid, username: &str) {
    let mut public_key_hash = [0_u8; 32];
    public_key_hash[..16].copy_from_slice(stable_id(username, "key").as_bytes());
    public_key_hash[16..].copy_from_slice(stable_id(username, "salt").as_bytes());
    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(username)
        .bind(public_key_hash.as_slice())
        .execute(pool)
        .await
        .expect("写入测试用户");
}

/// 把场次移出活动集合，允许同一世界创建下一轮场次。
///
/// draft 场次先删除 intake target；两条写入在同一事务里，延迟一致性触发器在提交时看到一致状态。
pub async fn retire_auction(store: &DatabaseStore, auction_id: Uuid) {
    let mut transaction = store.postgres_pool().begin().await.expect("开启回收事务");
    sqlx::query("DELETE FROM public.auction_intake_targets WHERE auction_id = $1")
        .bind(auction_id)
        .execute(&mut *transaction)
        .await
        .expect("回收 intake target");
    sqlx::query("UPDATE public.auctions SET status = 'settled' WHERE id = $1")
        .bind(auction_id)
        .execute(&mut *transaction)
        .await
        .expect("回收演练场次");
    transaction.commit().await.expect("提交回收事务");
}

/// 当前数据库里仍被持有的会话级 advisory lock 数；每个测试独占克隆库，因此只统计本夹具。
pub async fn advisory_lock_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks
         WHERE locktype = 'advisory'
           AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .fetch_one(pool)
    .await
    .expect("读取 advisory lock 数")
}

/// 在预算内等待条件成立；超时返回 `false`，由调用方断言给出可读失败信息。
pub async fn wait_for_condition<F, Fut>(budget: Duration, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = Instant::now() + budget;
    loop {
        if check().await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
