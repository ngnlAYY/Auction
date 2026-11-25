//! Take 起写的唯一许可与 fence 顺序（D9 §11.1 两个顺序）、起写与现金阻断的同事务原子性。

use std::sync::Arc;

use auction_application::consignment::{ConsignmentError, DepositBusinessRejection, WriteStart};
use auction_storage::DatabaseStore;
use sqlx::PgPool;

use super::super::deposit::support::succeeds;
use super::fixture::{
    Claim, Rts, acquire, claim_take, seed_refund_rts, service, start, take_delivery,
};
use crate::support::TestDatabase;

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取计数")
}

async fn starts(pool: &PgPool) -> i64 {
    count(pool, "SELECT count(*) FROM public.mail_disposition_write_starts WHERE work_id IN (SELECT take_work_id FROM public.consignment_return_intakes)").await
}

async fn open_blocks(pool: &PgPool) -> i64 {
    count(pool, "SELECT count(*) FROM public.consignment_cash_return_blocks WHERE state = 'open'")
        .await
}

async fn start_commands(pool: &PgPool) -> i64 {
    count(
        pool,
        "SELECT count(*) FROM public.consignment_commands WHERE operation = 'start_return_take'",
    )
    .await
}

async fn setup(database: &TestDatabase) -> (Arc<DatabaseStore>, DatabaseStore) {
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    seed_refund_rts(owner.postgres_pool(), &store, Rts { n: 1 }).await;
    (store, owner)
}

fn is_permit(start: &WriteStart) -> bool {
    matches!(start, WriteStart::FirstCommitted { .. })
}

/// 顺序一：A 未起写过租，B 取得新 fence。A 的旧起写得确定拒绝且零 POST；B 恰一次成功起写；重放无新增。
#[tokio::test]
async fn postgres_consignment_returns_order_one_stale_claim_is_rejected_and_new_fence_starts_once()
{
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let service = service(&store);
    let receipt = acquire(&service, Rts { n: 1 }).await.expect("接管");
    let delivery = take_delivery(pool, &receipt).await;
    let (claim_a, claim_b) = (Claim::new(21, 1, false), Claim::new(22, 2, true));
    claim_take(pool, &receipt, delivery, claim_a).await;
    claim_take(pool, &receipt, delivery, claim_b).await;

    let stale = start(&service, pool, &receipt, claim_a).await.expect_err("旧 fence 不得起写");
    assert!(matches!(
        stale,
        ConsignmentError::Deposit(DepositBusinessRejection::FenceMismatch)
            | ConsignmentError::LeaseLost
    ));
    assert_eq!((starts(pool).await, open_blocks(pool).await), (0, 0), "A 零起写、零阻断");
    assert_eq!(start_commands(pool).await, 1, "确定拒绝只占 A 自己的命令 ID");

    let first = start(&service, pool, &receipt, claim_b).await.expect("B 起写");
    assert!(is_permit(&first), "B 恰得一次许可");
    assert_eq!((starts(pool).await, open_blocks(pool).await), (1, 1));

    // 重放（同命令）与 A 的重放都不新增起写、不发许可。
    let replay = start(&service, pool, &receipt, claim_b).await.expect("B 重放");
    assert!(!is_permit(&replay));
    assert!(start(&service, pool, &receipt, claim_a).await.is_err());
    assert_eq!((starts(pool).await, open_blocks(pool).await), (1, 1));
}

/// 顺序二：A 已成功起写再过租，B 即使取得新 fence 也零新增 POST；A 原回执重放同样没有许可。
#[tokio::test]
async fn postgres_consignment_returns_order_two_started_work_gives_the_new_fence_no_permit() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let service = service(&store);
    let receipt = acquire(&service, Rts { n: 1 }).await.expect("接管");
    let delivery = take_delivery(pool, &receipt).await;
    let claim_a = Claim::new(21, 1, true);
    claim_take(pool, &receipt, delivery, claim_a).await;
    let first = start(&service, pool, &receipt, claim_a).await.expect("A 起写");
    assert!(is_permit(&first));

    // A 标记后、网络前中断：租约过期，B 取得新 fence。
    let claim_b = Claim::new(22, 2, true);
    claim_take(pool, &receipt, delivery, claim_b).await;
    let second = start(&service, pool, &receipt, claim_b).await.expect("B 只核对");
    assert!(!is_permit(&second), "已起写的工作不再发许可");
    let replay = start(&service, pool, &receipt, claim_a).await.expect("A 重放");
    assert!(!is_permit(&replay));
    assert_eq!((starts(pool).await, open_blocks(pool).await), (1, 1), "起写与阻断都只一条");
    let attempt: uuid::Uuid = sqlx::query_scalar(
        "SELECT attempt_id FROM public.mail_disposition_write_starts
         WHERE work_id IN (SELECT take_work_id FROM public.consignment_return_intakes)",
    )
    .fetch_one(pool)
    .await
    .expect("读取起写");
    assert_eq!(attempt, claim_a.attempt, "胜出的始终是 A 的 attempt");
}

/// 现金腿的起写与 UID 阻断同提交：阻断建立失败则起写一并回滚，之后同一命令可重试成功。
#[tokio::test]
async fn postgres_consignment_returns_cash_start_and_uid_block_commit_or_roll_back_together() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let service = service(&store);
    let receipt = acquire(&service, Rts { n: 1 }).await.expect("接管");
    let delivery = take_delivery(pool, &receipt).await;
    let claim = Claim::new(21, 1, true);
    claim_take(pool, &receipt, delivery, claim).await;
    succeeds(
        pool,
        "CREATE FUNCTION public.fixture_fail_block() RETURNS trigger LANGUAGE plpgsql AS
         $f$ BEGIN RAISE EXCEPTION '夹具：阻断写入失败'; END $f$;
         CREATE TRIGGER fixture_fail_block BEFORE INSERT ON public.consignment_cash_return_blocks
         FOR EACH ROW EXECUTE FUNCTION public.fixture_fail_block();",
    )
    .await;
    assert!(start(&service, pool, &receipt, claim).await.is_err(), "阻断失败时起写必须失败");
    assert_eq!((starts(pool).await, open_blocks(pool).await), (0, 0), "起写随之回滚");
    assert_eq!(start_commands(pool).await, 0, "回滚的命令不入账");

    succeeds(pool, "DROP TRIGGER fixture_fail_block ON public.consignment_cash_return_blocks;")
        .await;
    let retried = start(&service, pool, &receipt, claim).await.expect("同一命令重试");
    assert!(is_permit(&retried));
    assert_eq!((starts(pool).await, open_blocks(pool).await), (1, 1));
}
