//! 4B 实物强占用竞争：EID 规范化、跨世界隔离与无关实物并行提交。

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::Barrier;
use uuid::Uuid;

use crate::support::TestDatabase;

async fn insert_claim(
    pool: &PgPool,
    world: &str,
    eid: &str,
    barrier: Arc<Barrier>,
    delay: Duration,
) -> bool {
    let mut connection = pool.acquire().await.expect("取独立连接");
    sqlx::query("BEGIN").execute(&mut *connection).await.expect("开启事务");
    barrier.wait().await;
    tokio::time::sleep(delay).await;
    let inserted = sqlx::query(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ($1, public.consignment_eid_alias($2), 1, NULL, NULL,
                 transaction_timestamp(), $3)",
    )
    .bind(world)
    .bind(eid)
    .bind(Uuid::new_v4())
    .execute(&mut *connection)
    .await
    .is_ok();
    if inserted {
        sqlx::query("COMMIT").execute(&mut *connection).await.expect("提交胜者");
    } else {
        sqlx::query("ROLLBACK").execute(&mut *connection).await.expect("回滚败者");
    }
    inserted
}

async fn race(
    pool: &PgPool,
    first: (&'static str, &'static str),
    second: (&'static str, &'static str),
    first_delay: Duration,
    second_delay: Duration,
) -> (bool, bool) {
    let barrier = Arc::new(Barrier::new(2));
    let first_task = tokio::spawn({
        let pool = pool.clone();
        let barrier = barrier.clone();
        let (world, eid) = first;
        async move { insert_claim(&pool, world, eid, barrier, first_delay).await }
    });
    let second_task = tokio::spawn({
        let pool = pool.clone();
        let barrier = barrier.clone();
        let (world, eid) = second;
        async move { insert_claim(&pool, world, eid, barrier, second_delay).await }
    });
    (first_task.await.expect("第一条任务"), second_task.await.expect("第二条任务"))
}

async fn count(pool: &PgPool, world: &str, eid: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_equipment_claims
         WHERE world_id=$1 AND canonical_eid=public.consignment_eid_alias($2)",
    )
    .bind(world)
    .bind(eid)
    .fetch_one(pool)
    .await
    .expect("读取占用数")
}

#[tokio::test]
async fn postgres_claim_normalized_alias_conflicts_in_both_commit_orders() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    let owner = database.maintenance_store().await.expect("维护账号");
    let owner_pool = owner.postgres_pool();
    let first =
        race(pool, ("main", "42"), ("main", "00042"), Duration::ZERO, Duration::from_millis(20))
            .await;
    assert!(first.0 ^ first.1, "规范化同号只能一方提交");
    assert_eq!(count(pool, "main", "42").await, 1);
    sqlx::query("DELETE FROM public.consignment_equipment_claims WHERE world_id='main'")
        .execute(owner_pool)
        .await
        .expect("清理第一轮");
    let second =
        race(pool, ("main", "42"), ("main", "00042"), Duration::from_millis(20), Duration::ZERO)
            .await;
    assert!(second.0 ^ second.1, "交换提交顺序后仍只能一方提交");
    assert_eq!(count(pool, "main", "42").await, 1);
}

#[tokio::test]
async fn postgres_claim_same_eid_in_different_worlds_commits_both_orders() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    let owner = database.maintenance_store().await.expect("维护账号");
    let owner_pool = owner.postgres_pool();
    for (first_delay, second_delay) in
        [(Duration::ZERO, Duration::from_millis(20)), (Duration::from_millis(20), Duration::ZERO)]
    {
        let result = race(pool, ("main", "42"), ("isekai", "42"), first_delay, second_delay).await;
        assert_eq!(result, (true, true), "不同世界的同名 EID 不冲突");
        sqlx::query("DELETE FROM public.consignment_equipment_claims")
            .execute(owner_pool)
            .await
            .expect("清理跨世界用例");
    }
}

#[tokio::test]
async fn postgres_claim_unrelated_equipment_commits_in_parallel_both_orders() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    let owner = database.maintenance_store().await.expect("维护账号");
    let owner_pool = owner.postgres_pool();
    for (first_delay, second_delay) in
        [(Duration::ZERO, Duration::from_millis(20)), (Duration::from_millis(20), Duration::ZERO)]
    {
        let result = race(pool, ("main", "42"), ("main", "43"), first_delay, second_delay).await;
        assert_eq!(result, (true, true), "无关实物可并行提交");
        sqlx::query("DELETE FROM public.consignment_equipment_claims")
            .execute(owner_pool)
            .await
            .expect("清理无关实物用例");
    }
}
