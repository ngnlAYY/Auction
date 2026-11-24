//! 实物锁预算、失败释放与正常提交后的释放回归。

use std::assert_matches;
use std::time::Duration as StdDuration;

use auction_application::core::ApplicationError;
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_time::Timestamp;

use super::fixtures::{
    admin_command, admin_service, count_lots, equipment_key, hold_equipment_lock, mail_command,
    mail_service, seed_awaiting_without_reservation, seed_draft_target, seed_reservation_claim,
    seed_reservation_fixture, try_equipment_lock,
};
use crate::support::TestDatabase;

#[tokio::test]
async fn postgres_entry_releases_equipment_lock_after_held_path() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    seed_draft_target(pool, "main", 21_000).await;
    seed_reservation_claim(pool).await;
    let identity = ListingEquipmentIdentity::new("main", "42").expect("固定身份");
    let key = equipment_key(&identity);
    let now = Timestamp::now();
    let result =
        mail_service(store.clone(), now).ingest_mail(mail_command("main", &["42"], 1, now)).await;
    assert_matches!(
        result,
        Err(ApplicationError::CompetitionTransaction(
            auction_application::core::CompetitionTransactionError::EquipmentHeld
        ))
    );
    assert!(try_equipment_lock(pool, key).await, "取锁入口结束后实物锁必须已释放");
}

#[tokio::test]
async fn postgres_entry_lock_budget_failure_releases_already_acquired_equipment_locks() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 21_100).await;
    seed_reservation_claim(pool).await;
    let first = ListingEquipmentIdentity::new("main", "42").expect("固定身份");
    let second = ListingEquipmentIdentity::new("main", "43").expect("固定身份");
    let third = ListingEquipmentIdentity::new("main", "44").expect("固定身份");
    let mut ordered = [
        (equipment_key(&first), first),
        (equipment_key(&second), second),
        (equipment_key(&third), third),
    ];
    ordered.sort_by_key(|(key, _)| *key);
    let held_key = ordered[2].0;
    let free_keys = [ordered[0].0, ordered[1].0];
    let _holder = hold_equipment_lock(pool, held_key).await;

    let now = Timestamp::now();
    let result = mail_service(store.clone(), now)
        .ingest_mail(mail_command("main", &["42", "43", "44"], 2, now))
        .await;
    assert_matches!(
        result,
        Err(ApplicationError::CompetitionTransaction(
            auction_application::core::CompetitionTransactionError::EquipmentLockUnavailable
        ))
    );
    assert_eq!(count_lots(pool, auction_id).await, 0, "失败方必须零事实");
    for free_key in free_keys {
        let free_released = tokio::time::timeout(StdDuration::from_secs(10), async {
            loop {
                if try_equipment_lock(pool, free_key).await {
                    break true;
                }
                tokio::time::sleep(StdDuration::from_millis(10)).await;
            }
        })
        .await;
        if free_released.is_err() {
            let rows = sqlx::query_as::<_, (i32, i64, i64, bool)>(
                "SELECT pid, classid::bigint, objid::bigint, granted
                 FROM pg_locks WHERE locktype='advisory' ORDER BY pid, classid, objid",
            )
            .fetch_all(pool)
            .await
            .unwrap_or_default();
            panic!("失败的锁租约必须关闭会话并释放已取得的锁; locks={rows:?}");
        }
        assert!(free_released.unwrap_or(false), "已取得的其他实物锁必须释放");
    }
    assert!(!try_equipment_lock(pool, held_key).await, "仍被另一连接持有");
}

#[tokio::test]
async fn postgres_entries_for_unrelated_equipment_both_commit() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 21_200).await;
    seed_reservation_fixture(pool).await;
    let now = Timestamp::now();
    let left_store = store.clone();
    let left = async move {
        mail_service(left_store, now).ingest_mail(mail_command("main", &["42"], 3, now)).await
    };
    let right_store = store.clone();
    let right = async move {
        mail_service(right_store, now).ingest_mail(mail_command("main", &["43"], 4, now)).await
    };
    let (left, right) = tokio::join!(left, right);
    left.expect("无关实物 42 应提交");
    right.expect("无关实物 43 应提交");
    assert_eq!(count_lots(pool, auction_id).await, 2);
}

#[tokio::test]
async fn postgres_mail_entry_skips_unclaimed_equipment_lock_even_when_held() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 21_300).await;
    seed_awaiting_without_reservation(pool).await;
    let identity = ListingEquipmentIdentity::new("main", "42").expect("固定身份");
    let _holder = hold_equipment_lock(pool, equipment_key(&identity)).await;
    let now = Timestamp::now();
    mail_service(store.clone(), now)
        .ingest_mail(mail_command("main", &["42"], 5, now))
        .await
        .expect("无寄售持有时不得取实物锁，外部会话锁不应阻塞入站");
    assert_eq!(count_lots(pool, auction_id).await, 1);
}

#[tokio::test]
async fn postgres_admin_entry_skips_unclaimed_equipment_lock_even_when_held() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 21_400).await;
    seed_awaiting_without_reservation(pool).await;
    let identity = ListingEquipmentIdentity::new("main", "42").expect("固定身份");
    let _holder = hold_equipment_lock(pool, equipment_key(&identity)).await;
    let now = Timestamp::now();
    admin_service(store.clone(), "42", now)
        .add_item(admin_command("main", "42", now))
        .await
        .expect("无寄售持有时不得取实物锁，外部会话锁不应阻塞录入");
    assert_eq!(count_lots(pool, auction_id).await, 1);
}
