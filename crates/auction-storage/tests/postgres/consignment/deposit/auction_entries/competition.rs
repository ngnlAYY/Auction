//! 预约签发与拍卖邮件/Admin 录入的真实入口竞争：显式 barrier、交换提交顺序、恰好一方成功。
//!
//! 恢复重列与同一 EID 的预约签发不能构成“双方都有资格”的真实竞争：`check_listing_history`
//! 在存在待重列 lot/恢复责任时必先拒绝该 EID 的新登记/签发。恢复入口的强占用阻断、别名与
//! 无占用重列由 `recovery.rs` 的确定性用例直接覆盖。

use std::assert_matches;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use auction_application::consignment::{IssueReservationCommand, ReservationStore};
use auction_application::core::{ApplicationError, CompetitionTransactionError, RequestHash};
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_domain::consignment::ListingVersion;
use auction_domain::{CommandId, UserId};
use auction_storage::{DatabaseStore, TestDepositLocks};
use auction_time::Timestamp;
use sqlx::PgPool;
use tokio::sync::Barrier;
use uuid::Uuid;

use super::super::support::{LISTING, USER_1};
use super::fixtures::{
    admin_command, admin_service, count_lots, deposit_repository, mail_command, mail_service,
    seed_awaiting_without_reservation, seed_draft_target, seed_reservation_claim,
};
use crate::support::TestDatabase;

#[derive(Clone, Copy)]
enum Entry {
    Mail,
    Admin,
}

fn issue_command(seed: u8) -> IssueReservationCommand {
    IssueReservationCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        seller_user_id: UserId::from_uuid(Uuid::parse_str(USER_1).expect("固定卖家")),
        listing_id: Uuid::parse_str(LISTING).expect("固定挂单"),
        expected_version: ListingVersion::new(2).expect("挂单版本"),
        request_hash: RequestHash::new([seed; 32]),
        request_id: None,
    }
}

async fn race_one(entry_kind: Entry, reservation_first: bool) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    seed_awaiting_without_reservation(pool).await;
    let auction_id = seed_draft_target(pool, "main", 22_000).await;
    let barrier = Arc::new(Barrier::new(2));

    let repository = deposit_repository(store.clone());
    let reservation_barrier = Arc::clone(&barrier);
    let reservation = tokio::spawn(async move {
        reservation_barrier.wait().await;
        if !reservation_first {
            tokio::time::sleep(StdDuration::from_millis(200)).await;
        }
        repository.issue_reservation(issue_command(9)).await.is_ok()
    });

    let entry_barrier = Arc::clone(&barrier);
    let entry_store = store.clone();
    let now = Timestamp::now();
    let entry = tokio::spawn(async move {
        entry_barrier.wait().await;
        if reservation_first {
            tokio::time::sleep(StdDuration::from_millis(200)).await;
        }
        match entry_kind {
            Entry::Mail => mail_service(entry_store, now)
                .ingest_mail(mail_command("main", &["42"], 10, now))
                .await
                .is_ok(),
            Entry::Admin => admin_service(entry_store, "42", now)
                .add_item(admin_command("main", "42", now))
                .await
                .is_ok(),
        }
    });

    let (reservation_created, entry_created) =
        tokio::time::timeout(StdDuration::from_secs(15), async {
            (reservation.await.expect("预约任务"), entry.await.expect("入口任务"))
        })
        .await
        .expect("两侧必须在预算内完成，不得死锁");
    assert!(
        reservation_created ^ entry_created,
        "恰好一方成功；预约={reservation_created}，入口={entry_created}"
    );

    let lots: i64 = sqlx::query_scalar("SELECT count(*) FROM public.lots WHERE auction_id=$1")
        .bind(auction_id)
        .fetch_one(pool)
        .await
        .expect("读取 lot");
    let active_reservations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_deposit_reservations
         WHERE listing_id=$1 AND state='active'",
    )
    .bind(Uuid::parse_str(LISTING).expect("固定挂单"))
    .fetch_one(pool)
    .await
    .expect("读取预约");
    if entry_created {
        assert_eq!(lots, 1);
        assert_eq!(active_reservations, 0, "失败方零事实");
    } else {
        assert_eq!(lots, 0, "失败方零事实");
        assert_eq!(active_reservations, 1);
    }
}

#[tokio::test]
async fn postgres_reservation_races_mail_admin_and_recovery_relist_through_entries() {
    for entry in [Entry::Mail, Entry::Admin] {
        for reservation_first in [true, false] {
            race_one(entry, reservation_first).await;
        }
    }
}

fn equipment_key(identity: &ListingEquipmentIdentity) -> (i32, i32) {
    DatabaseStore::test_deposit_lock_keys(&TestDepositLocks {
        command_id: Uuid::nil(),
        sellers: Vec::new(),
        equipment: vec![identity.clone()],
        mail_keys: Vec::new(),
    })
    .last()
    .copied()
    .expect("命令与实物锁键至少各一把")
}

async fn equipment_lock_held(pool: &PgPool, key: (i32, i32)) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM pg_locks
             WHERE locktype='advisory' AND granted
               AND database = (SELECT oid FROM pg_database
                               WHERE datname = current_database())
               AND classid::bigint = $1 AND objid::bigint = $2
         )",
    )
    .bind(i64::from(key.0 as u32))
    .bind(i64::from(key.1 as u32))
    .fetch_one(pool)
    .await
    .expect("读取 advisory lock")
}

#[tokio::test]
async fn postgres_mail_entry_takes_equipment_lock_before_writing() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    seed_reservation_claim(pool).await;
    let auction_id = seed_draft_target(pool, "main", 22_100).await;
    let identity = ListingEquipmentIdentity::new("main", "42").expect("固定身份");
    let key = equipment_key(&identity);

    // 门控看到强占用后会先取实物租约，再在事务内锁 world；用 world 行锁把入口停在取锁之后。
    let mut world_lock = pool.begin().await.expect("world 行锁事务");
    sqlx::query("SELECT id FROM public.worlds WHERE id='main' FOR UPDATE")
        .execute(&mut *world_lock)
        .await
        .expect("锁住 world 行");

    let now = Timestamp::now();
    let mail_store = store.clone();
    let mail = tokio::spawn(async move {
        mail_service(mail_store, now).ingest_mail(mail_command("main", &["42"], 11, now)).await
    });
    let acquired = tokio::time::timeout(StdDuration::from_secs(2), async {
        loop {
            if equipment_lock_held(pool, key).await {
                break true;
            }
            tokio::time::sleep(StdDuration::from_millis(10)).await;
        }
    })
    .await
    .expect("拍卖入口必须在写 lot 前取得实物锁");
    assert!(acquired);

    world_lock.rollback().await.expect("释放 world 行锁");
    let result = mail.await.expect("拍卖入口");
    assert_matches!(
        result,
        Err(ApplicationError::CompetitionTransaction(CompetitionTransactionError::EquipmentHeld)),
        "已存在的强占用必须得到既定的 EquipmentHeld"
    );
    assert_eq!(count_lots(pool, auction_id).await, 0, "强占用失败方零事实");
}
