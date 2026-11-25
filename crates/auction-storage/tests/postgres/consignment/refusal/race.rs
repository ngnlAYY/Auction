//! 买家锁：拒收与购买阻断读共用同一把买家互斥，顺序可交换且阻断自提交起生效；缺锁失败关闭。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use auction_storage::TestAfterLock;
use tokio::sync::Notify;
use tokio::time::sleep;

use super::fixture::{
    buyer, command, count, good_evidence, seed_base, seed_intakes, seller, submission, submit,
};
use crate::support::TestDatabase;

/// 取锁后停住、直到 `release` 被通知才继续开快照的钩子；`entered` 在停住时通知。
fn pause_hook(entered: Arc<Notify>, release: Arc<Notify>) -> Box<TestAfterLock> {
    Box::new(move || -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let (entered, release) = (entered.clone(), release.clone());
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
        })
    })
}

/// 两条拒收已提交、第三份尚未提交的现场，第三份一提交就触发处罚。
async fn two_submitted() -> (TestDatabase, Arc<auction_storage::DatabaseStore>, sqlx::PgPool) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool().clone();
    let store = Arc::new(database.store().await.expect("以运行账号连接"));
    seed_base(&pool).await;
    seed_intakes(&pool, 3).await;
    submit(&store, 1).await;
    submit(&store, 2).await;
    (database, store, pool)
}

#[tokio::test]
async fn postgres_consignment_refusal_purchase_holding_buyer_lock_delays_sanction_commit() {
    let (_database, store, pool) = two_submitted().await;
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));

    // 购买先取到买家锁并停在开快照之前；拒收提交必须等它，处罚不能抢在购买重验之前提交。
    let purchase = {
        let (store, entered, release) = (store.clone(), entered.clone(), release.clone());
        tokio::spawn(async move {
            let hook = pause_hook(entered, release);
            store.test_purchase_gate(command(500), buyer(), seller(), Some(&*hook)).await
        })
    };
    entered.notified().await;
    let refusal = {
        let store = store.clone();
        tokio::spawn(async move {
            store
                .test_refusal_submit(
                    command(501),
                    &[buyer()],
                    &submission(3, good_evidence()),
                    None,
                )
                .await
        })
    };
    sleep(Duration::from_millis(250)).await;
    assert!(!refusal.is_finished(), "拒收事务应被买家锁挡住");
    assert_eq!(count(&pool, "consignment_sanctions").await, 0);

    release.notify_one();
    let gate = purchase.await.expect("购买任务").expect("阻断读");
    assert!(!gate, "购买在处罚提交之前重验，不应看到阻断");
    let outcome = refusal.await.expect("拒收任务").expect("拒收提交");
    assert!(outcome.sanction.is_some());
}

#[tokio::test]
async fn postgres_consignment_refusal_commit_blocks_the_next_purchase_from_its_commit() {
    let (_database, store, pool) = two_submitted().await;
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));

    // 拒收先取到买家锁并停住；随后的购买重验必须等拒收提交，再读到阻断。
    let refusal = {
        let (store, entered, release) = (store.clone(), entered.clone(), release.clone());
        tokio::spawn(async move {
            let hook = pause_hook(entered, release);
            store
                .test_refusal_submit(
                    command(600),
                    &[buyer()],
                    &submission(3, good_evidence()),
                    Some(&*hook),
                )
                .await
        })
    };
    entered.notified().await;
    let purchase = {
        let store = store.clone();
        tokio::spawn(async move {
            store.test_purchase_gate(command(601), buyer(), seller(), None).await
        })
    };
    sleep(Duration::from_millis(250)).await;
    assert!(!purchase.is_finished(), "购买重验应被买家锁挡住");

    release.notify_one();
    let outcome = refusal.await.expect("拒收任务").expect("拒收提交");
    assert!(outcome.sanction.is_some());
    assert!(purchase.await.expect("购买任务").expect("阻断读"), "阻断应自处罚提交起生效");
    assert_eq!(count(&pool, "consignment_sanctions").await, 1);
}

#[tokio::test]
async fn postgres_consignment_refusal_without_buyer_lock_fails_closed() {
    let (_database, store, pool) = two_submitted().await;
    // 锁集合里没有买家（只有卖家）：原语拒绝执行，不写任何行。
    let result = store
        .test_refusal_submit(command(700), &[seller()], &submission(3, good_evidence()), None)
        .await;
    assert!(result.is_err());
    assert_eq!(count(&pool, "consignment_return_facts").await, 2);
    assert_eq!(count(&pool, "consignment_sanctions").await, 0);
}
