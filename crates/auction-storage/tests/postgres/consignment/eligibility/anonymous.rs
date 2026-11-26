//! 匿名非寄售挂单（迁移 000138）：卖家二选一约束、同名同件唯一、同名账户资格与按名计数。

use sqlx::{AssertSqlSafe, PgConnection};
use uuid::Uuid;

use super::fixtures::{END, REPLAY, START, at, commit, gear, seed};
use super::production::accounts::{lock_listing_seller, lock_same_name_account, same_game_name};
use super::production::occupancy::{
    anonymous_activity_count, anonymous_hourly_registration_count, anonymous_resolving_count,
    lock_equipment_occupants,
};
use crate::support::TestDatabase;

/// 与生产登记相同的首写形状：无账户、自填名、`anonymous` 命令与同版事件。
fn anonymous_registration(listing_id: Uuid, name: &str, eid: &str, mode: &str) -> String {
    let command_id = Uuid::new_v4();
    let fee = if mode == "direct" { 0 } else { 500 };
    format!(
        r#"
        INSERT INTO public.consignment_listings
            (id,seller_user_id,seller_game_name,world_id,external_equipment_id,mode,state,price,
             pricing_version,fee_bps,state_version,service_epoch,blacklist_count_at_register,
             blacklist_baseline_ids,created_at,updated_at,expires_at)
        VALUES ('{listing_id}',NULL,'{name}','main','{eid}','{mode}','resolving',101,1,{fee},1,1,0,
                '{{}}'::uuid[],'{START}','{START}','{END}');
        INSERT INTO public.consignment_commands
            (command_id,actor_kind,actor_ref,operation,request_target,request_digest,replay_key_id,
             listing_id,result_status,result_code,result_body,created_at)
        VALUES ('{command_id}','anonymous','{name}','register','new',decode(repeat('11',32),'hex'),
                '{REPLAY}','{listing_id}',202,'accepted',
                '{{"command_id":"{command_id}","operation":"register","listing_id":"{listing_id}",
                   "listing_version":1,"state":"resolving","disposition":"registered","price":101}}',
                '{START}');
        INSERT INTO public.consignment_events
            (listing_id,state_version,command_id,actor_kind,event_kind,to_state,price,occurred_at)
        VALUES ('{listing_id}',1,'{command_id}','anonymous','registered','resolving',101,'{START}');
    "#
    )
}

async fn rejected_constraint(cx: &mut PgConnection, sql: String) -> Option<String> {
    let error = sqlx::raw_sql(AssertSqlSafe(sql)).execute(cx).await.expect_err("约束应拒绝写入");
    error.as_database_error().and_then(|error| error.constraint()).map(str::to_owned)
}

#[tokio::test]
async fn postgres_anonymous_direct_listing_registers_and_counts_by_game_name() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    seed(owner.postgres_pool()).await;
    let listing_id = Uuid::from_u128(901);
    let mut tx = runtime.postgres_pool().begin().await.expect("匿名登记事务");
    sqlx::raw_sql(AssertSqlSafe(anonymous_registration(listing_id, "Anon Seller", "42", "direct")))
        .execute(&mut *tx)
        .await
        .expect("运行账号可写入匿名非寄售挂单");
    commit(tx).await;

    let mut tx = runtime.postgres_pool().begin().await.expect("读取事务");
    let as_of = at("2026-01-01T00:30:00Z");
    let occupants =
        lock_equipment_occupants(&mut tx, &gear("main", "42"), as_of).await.expect("占用");
    assert_eq!(occupants.len(), 1);
    assert_eq!(occupants[0].seller_user_id, None);
    assert_eq!(occupants[0].seller_game_key.as_deref(), Some("anon seller"));
    // 名称按 lower 等值计数，大小写不同仍属同一自填名。
    let hourly = anonymous_hourly_registration_count(&mut tx, "ANON seller", as_of).await;
    assert_eq!(hourly.expect("小时计数"), 1);
    assert_eq!(anonymous_activity_count(&mut tx, "anon SELLER", as_of).await.expect("活动计数"), 1);
    assert_eq!(anonymous_resolving_count(&mut tx, as_of).await.expect("全站解析计数"), 1);
    assert_eq!(anonymous_resolving_count(&mut tx, at(END)).await.expect("截止即失效"), 0);
    // 同名账户（seed 的 FakeSeller）按 lower 命中；无同名账户时视为合格且不受拉黑约束。
    assert!(lock_same_name_account(&mut tx, "fakeseller").await.expect("同名账户").is_some());
    let unnamed = lock_listing_seller(&mut tx, Uuid::nil(), Some("Anon Seller")).await;
    let unnamed = unnamed.expect("匿名资格").expect("匿名卖家总有资格行");
    assert!(unnamed.active && !unnamed.active_blacklist && unnamed.blacklist_count == 0);
    assert!(same_game_name(&mut tx, "Anon Seller", "anon seller").await.expect("名称比较"));
    assert!(!same_game_name(&mut tx, "Anon Seller", "Other").await.expect("名称比较"));
    commit(tx).await;
}

#[tokio::test]
async fn postgres_anonymous_listing_rejects_consigned_mode_and_same_name_duplicate() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    seed(owner.postgres_pool()).await;
    let pool = runtime.postgres_pool();

    let mut tx = pool.begin().await.expect("匿名寄售事务");
    let consigned = anonymous_registration(Uuid::from_u128(911), "Anon", "42", "consigned");
    let constraint = rejected_constraint(&mut tx, consigned).await;
    assert_eq!(constraint.as_deref(), Some("consignment_listing_seller"));
    tx.rollback().await.expect("回滚");

    let mut tx = pool.begin().await.expect("首个匿名登记");
    sqlx::raw_sql(AssertSqlSafe(anonymous_registration(
        Uuid::from_u128(912),
        "Anon",
        "42",
        "direct",
    )))
    .execute(&mut *tx)
    .await
    .expect("首个匿名挂单");
    commit(tx).await;
    let mut tx = pool.begin().await.expect("同名同件登记");
    let duplicate = anonymous_registration(Uuid::from_u128(913), "ANON", "42", "direct");
    let constraint = rejected_constraint(&mut tx, duplicate).await;
    assert_eq!(constraint.as_deref(), Some("consignment_listings_anonymous_active_idx"));
    tx.rollback().await.expect("回滚");
}
