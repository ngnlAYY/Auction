//! v2 K 锁组、锁序、快照前取锁、同连接事务与多挂单原子回滚。

use std::sync::Arc;
use std::time::Duration;

use auction_domain::SettlementWorld;
use auction_domain::consignment::{ListingEquipmentIdentity, OriginalMailKey};
use auction_storage::{
    DatabaseStore, TestDepositLocks, test_deposit_command_exists, test_deposit_event_count,
    test_deposit_listing_state, test_deposit_marker_exists,
};
use serde_json::Value;
use sqlx::Row;
use tokio::sync::Barrier;
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

use super::support::{
    END, LATER, LISTING, START, USER_1, execute, listing_awaiting_deposit, users_and_key,
};
use crate::support::TestDatabase;

const SECOND_LISTING: &str = "21000000-0000-0000-0000-000000000002";
const SECOND_SNAPSHOT: &str = "31000000-0000-0000-0000-000000000002";
const SECOND_COMMAND: &str = "41000000-0000-5000-8000-000000000092";

fn locks(command_id: Uuid) -> TestDepositLocks {
    TestDepositLocks {
        command_id,
        sellers: vec![Uuid::parse_str(USER_1).expect("卖家")],
        equipment: vec![ListingEquipmentIdentity::new("main", "42").expect("装备")],
        mail_keys: vec![mail_key("12345", "555")],
    }
}

fn mail_key(bot_uid: &str, mid: &str) -> OriginalMailKey {
    OriginalMailKey::new(bot_uid, SettlementWorld::Main, mid).expect("原信键")
}

async fn seed() -> TestDatabase {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接运行账号");
    execute(store.postgres_pool(), &users_and_key()).await.expect("写入用户");
    execute(store.postgres_pool(), &listing_awaiting_deposit()).await.expect("写入挂单");
    execute(store.postgres_pool(), &second_listing()).await.expect("写入第二挂单");
    database
}

fn second_listing() -> String {
    format!(
        "INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
             blacklist_baseline_ids, created_at, updated_at, expires_at)
         VALUES ('{SECOND_LISTING}', '10000000-0000-0000-0000-000000000002', 'main', '43',
                 'consigned', 'resolving', 101, 1, 500, 1, 1, 0,
                 ARRAY[]::uuid[], '{START}', '{START}', '{END}');
         INSERT INTO public.consignment_snapshots
            (id, listing_id, generation, adapter, observed_at, digest, world_id,
             external_equipment_id, name, category, alt_name, level_kind, level_number,
             tradeable, owner_name, stat_groups)
         VALUES ('{SECOND_SNAPSHOT}', '{SECOND_LISTING}', 1, 'hvdata-equip-v1', '{LATER}',
                 decode(repeat('91', 32), 'hex'), 'main', '43', 'Fake Gear Two', 'Weapon',
                 NULL, 'assigned', 100, true, 'FakeSellerTwo', '[]'::jsonb);
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('41000000-0000-4000-8000-000000000091', 'public_user',
                 '10000000-0000-0000-0000-000000000002', 'register', 'new',
                 decode(repeat('93', 32), 'hex'), '90000000-0000-0000-0000-000000000001',
                 '{SECOND_LISTING}', 202, 'accepted',
                 '{{\"command_id\":\"41000000-0000-4000-8000-000000000091\",
                    \"operation\":\"register\",\"listing_id\":\"{SECOND_LISTING}\",
                    \"listing_version\":1,\"state\":\"resolving\",
                    \"disposition\":\"registered\",\"price\":101}}', '{START}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{SECOND_LISTING}', 1, '41000000-0000-4000-8000-000000000091',
                 'public_user', 'registered', NULL, 'resolving', NULL, 101, '{START}');
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{SECOND_COMMAND}', 'worker', 'fake-worker', 'apply_lookup', '{SECOND_LISTING}',
                 decode(repeat('92', 32), 'hex'), NULL, '{SECOND_LISTING}', 200, 'ok',
                 '{{\"command_id\":\"{SECOND_COMMAND}\",\"operation\":\"apply_lookup\",
                    \"listing_id\":\"{SECOND_LISTING}\",\"listing_version\":2,
                    \"state\":\"awaiting_deposit\",\"disposition\":\"published\",\"price\":101,
                    \"expires_at\":\"{END}\",\"reason_code\":null}}', '{LATER}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{SECOND_LISTING}', 2, '{SECOND_COMMAND}', 'worker', 'published', 'resolving',
                 'awaiting_deposit', NULL, 101, '{LATER}');
         UPDATE public.consignment_listings
         SET state='awaiting_deposit', state_version=2, current_snapshot_id='{SECOND_SNAPSHOT}',
             updated_at='{LATER}'
         WHERE id='{SECOND_LISTING}';"
    )
}

/// 锁组顺序固定为命令、排序去重的用户、规范实物，K 始终最后。
#[test]
fn postgres_deposit_lock_order_is_command_user_equipment_then_mail_key() {
    let locks = TestDepositLocks {
        command_id: Uuid::from_u128(1),
        sellers: vec![Uuid::from_u128(3), Uuid::from_u128(2), Uuid::from_u128(2)],
        equipment: vec![
            ListingEquipmentIdentity::new("isekai", "42").expect("装备"),
            ListingEquipmentIdentity::new("main", "43").expect("装备"),
            ListingEquipmentIdentity::new("main", "42").expect("装备"),
        ],
        mail_keys: vec![mail_key("22345", "556"), mail_key("12345", "555")],
    };
    let labels = auction_storage::DatabaseStore::test_deposit_lock_order(&locks);
    assert_eq!(labels[0], "command");
    let first_equipment = labels.iter().position(|label| *label == "equipment").expect("实物锁");
    let first_mail_key = labels.iter().position(|label| *label == "mail_key").expect("K 锁");
    assert!(labels[1..first_equipment].iter().all(|label| *label == "user"));
    assert!(labels[first_equipment..first_mail_key].iter().all(|label| *label == "equipment"));
    assert!(labels[first_mail_key..].iter().all(|label| *label == "mail_key"));
}

/// 两个连接反向传入同一组用户 / 实物 / K，必须按规范顺序取得而不会死锁。
#[tokio::test]
async fn postgres_deposit_lock_groups_are_deadlock_free_in_reverse_order() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    let mut first = TestDepositLocks {
        command_id: Uuid::from_u128(0x7101),
        sellers: vec![Uuid::from_u128(0x7201), Uuid::from_u128(0x7202)],
        equipment: vec![
            ListingEquipmentIdentity::new("main", "42").expect("装备"),
            ListingEquipmentIdentity::new("isekai", "43").expect("装备"),
        ],
        mail_keys: vec![mail_key("12345", "555"), mail_key("12346", "556")],
    };
    let mut second = first.clone();
    second.command_id = Uuid::from_u128(0x7102);
    first.sellers.reverse();
    first.equipment.reverse();
    first.mail_keys.reverse();
    let barrier = Arc::new(Barrier::new(2));
    let left = {
        let store = store.clone();
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move { store.test_deposit_hold_locks(first, barrier).await })
    };
    let right = {
        let store = store.clone();
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move { store.test_deposit_hold_locks(second, barrier).await })
    };
    let (left, right) = timeout(Duration::from_secs(10), async {
        (left.await.expect("左任务"), right.await.expect("右任务"))
    })
    .await
    .expect("规范锁序不应死锁");
    left.expect("左事务");
    right.expect("右事务");
}

/// 锁在快照前取得：另一连接在取锁后提交，本事务的第一条读语句必须看见它。
#[tokio::test]
async fn postgres_deposit_locks_are_acquired_before_the_snapshot() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接运行账号");
    execute(store.postgres_pool(), &users_and_key()).await.expect("写入用户");
    let probe = Uuid::from_u128(0x7301);
    let acquired = Arc::new(Barrier::new(2));
    let committed = Arc::new(Barrier::new(2));
    let task = {
        let store = store.clone();
        let acquired = Arc::clone(&acquired);
        let committed = Arc::clone(&committed);
        tokio::spawn(async move {
            store
                .test_deposit_locks_before_snapshot(
                    locks(Uuid::from_u128(0x7302)),
                    probe,
                    acquired,
                    committed,
                )
                .await
        })
    };
    acquired.wait().await;
    execute(
        store.postgres_pool(),
        &format!(
            "INSERT INTO public.consignment_replay_keys
                (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
             VALUES ('{probe}', false, decode(repeat('93', 48), 'hex'),
                     decode(repeat('94', 12), 'hex'), 'fake-data-key', '{START}', '{START}')"
        ),
    )
    .await
    .expect("另一连接提交");
    committed.wait().await;
    let observed = timeout(Duration::from_secs(10), task)
        .await
        .expect("快照读取不应超时")
        .expect("任务")
        .expect("快照读取");
    assert_eq!(observed, 1, "取锁后提交的行必须在随后开启的快照中可见");
}

/// 寄售事务里的旧原语使用同一 `TrackedTransaction`；整体回滚时两边写入都不留。
#[tokio::test]
async fn postgres_deposit_same_connection_transaction_rolls_back_both_writes() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    let committed_marker = Uuid::from_u128(0x7401);
    let committed_business = Uuid::from_u128(0x7405);
    store
        .test_deposit_same_connection_marker(
            locks(Uuid::from_u128(0x7402)),
            committed_marker,
            committed_business,
            true,
        )
        .await
        .expect("提交路径");
    assert!(test_deposit_marker_exists(&store, committed_marker).await);
    assert!(test_deposit_marker_exists(&store, committed_business).await);

    let rolled_back_marker = Uuid::from_u128(0x7403);
    let rolled_back_business = Uuid::from_u128(0x7406);
    store
        .test_deposit_same_connection_marker(
            locks(Uuid::from_u128(0x7404)),
            rolled_back_marker,
            rolled_back_business,
            false,
        )
        .await
        .expect_err("强制回滚");
    assert!(
        !test_deposit_marker_exists(&store, rolled_back_marker).await,
        "同一顶层事务回滚后，旧原语与业务写入都不能留下"
    );
    assert!(
        !test_deposit_marker_exists(&store, rolled_back_business).await,
        "同一顶层事务回滚后，业务写入也不能留下"
    );
}

/// 一条命令推进两个挂单；其中一件版本冲突时整条命令回滚、零事件。
#[tokio::test]
async fn postgres_deposit_multi_listing_command_is_atomic() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    let first = Uuid::parse_str(LISTING).expect("挂单");
    let second = Uuid::parse_str(SECOND_LISTING).expect("第二挂单");
    let success = Uuid::new_v5(&Uuid::NAMESPACE_URL, b"deposit-multi-listing-command");
    store
        .test_deposit_advance_two_listings(success, first, second, 2, 2)
        .await
        .expect("两件同时推进");
    assert_eq!(
        test_deposit_listing_state(&store, first).await.expect("第一件"),
        ("deposit_accepted".into(), 3)
    );
    assert_eq!(
        test_deposit_listing_state(&store, second).await.expect("第二件"),
        ("deposit_accepted".into(), 3)
    );
    assert_eq!(test_deposit_event_count(&store, success).await.expect("事件数"), 2);
    assert!(test_deposit_command_exists(&store, success).await);
    let events = sqlx::query(
        "SELECT listing_id, state_version, to_state
         FROM public.consignment_events WHERE command_id = $1 ORDER BY listing_id",
    )
    .bind(success)
    .fetch_all(store.postgres_pool())
    .await
    .expect("读取事件");
    assert_eq!(events.len(), 2);
    for (row, expected_listing) in events.iter().zip([first, second]) {
        assert_eq!(row.get::<Uuid, _>("listing_id"), expected_listing);
        assert_eq!(row.get::<i64, _>("state_version"), 3);
        assert_eq!(row.get::<String, _>("to_state"), "deposit_accepted");
    }
    let receipt: Value = sqlx::query_scalar(
        "SELECT result_body FROM public.consignment_commands WHERE command_id = $1",
    )
    .bind(success)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取回执");
    let changes = receipt["listing_changes"].as_array().expect("逐件 L");
    assert_eq!(changes.len(), 2);
    for (change, expected_listing) in changes.iter().zip([first, second]) {
        let expected_listing = expected_listing.to_string();
        assert_eq!(change["listing_id"].as_str(), Some(expected_listing.as_str()));
        assert_eq!(change["listing_version"], 3);
        assert_eq!(change["state"], "deposit_accepted");
    }

    let conflict = Uuid::new_v5(&Uuid::NAMESPACE_URL, b"deposit-multi-listing-conflict");
    let error = store
        .test_deposit_advance_two_listings(conflict, first, second, 3, 2)
        .await
        .expect_err("第二件版本冲突");
    assert!(error.contains("StaleVersion"), "第二件应因版本冲突被拒绝：{error}");
    assert_eq!(
        test_deposit_listing_state(&store, first).await.expect("第一件保持"),
        ("deposit_accepted".into(), 3)
    );
    assert_eq!(
        test_deposit_listing_state(&store, second).await.expect("第二件保持"),
        ("deposit_accepted".into(), 3)
    );
    assert_eq!(test_deposit_event_count(&store, conflict).await.expect("事件数"), 0);
    assert!(!test_deposit_command_exists(&store, conflict).await);
}

/// 事务内的 `pg_backend_pid()` 必须就是持有命令、用户、实物与 K 锁的会话。
#[tokio::test]
async fn postgres_deposit_locks_belong_to_the_transaction_session() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    store
        .test_deposit_locks_owned_by_transaction_session(locks(Uuid::from_u128(0x7501)))
        .await
        .expect("advisory 锁必须由同一事务会话持有");
}

/// 另一连接占住 K 时，生产事务在锁预算内失败，并释放已取得的其他组锁。
#[tokio::test]
async fn postgres_deposit_k_lock_blocks_and_releases_earlier_groups() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    let locks = locks(Uuid::from_u128(0x7601));
    let keys = DatabaseStore::test_deposit_lock_keys(&locks);
    let (mail_namespace, mail_key) = *keys.last().expect("K 锁必须最后");

    let mut holder = store.postgres_pool().acquire().await.expect("占用 K 的连接");
    sqlx::query("SELECT pg_advisory_lock($1, $2)")
        .bind(mail_namespace)
        .bind(mail_key)
        .execute(&mut *holder)
        .await
        .expect("另一连接先持有 K");

    let error =
        store.test_deposit_attempt_locks(locks).await.expect_err("K 被占用时寄售事务必须失败");
    assert!(error.contains("Unavailable"), "应在锁预算内失败为 Unavailable：{error}");

    // 失败路径必须关闭专用连接，从而释放命令、用户与实物锁。
    let mut probe = store.postgres_pool().acquire().await.expect("第三连接");
    let deadline = Instant::now() + Duration::from_secs(5);
    for &(namespace, key) in &keys[..keys.len() - 1] {
        loop {
            let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
                .bind(namespace)
                .bind(key)
                .fetch_one(&mut *probe)
                .await
                .expect("尝试取得先前组的锁");
            if acquired {
                break;
            }
            assert!(Instant::now() < deadline, "K 阻塞失败后未释放先前组锁 {namespace}/{key}");
            sleep(Duration::from_millis(25)).await;
        }
    }
    for &(namespace, key) in &keys[..keys.len() - 1] {
        let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1, $2)")
            .bind(namespace)
            .bind(key)
            .fetch_one(&mut *probe)
            .await
            .expect("释放探针锁");
        assert!(released, "探针应持有先前组锁 {namespace}/{key}");
    }
    let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1, $2)")
        .bind(mail_namespace)
        .bind(mail_key)
        .fetch_one(&mut *holder)
        .await
        .expect("释放 K");
    assert!(released, "占用连接应持有 K");
}

/// 会话级超时要求 v2 命令连接不回池：命令结束后后端 pid 必须离开 `pg_stat_activity`。
#[tokio::test]
async fn postgres_deposit_command_connection_leaves_pg_stat_activity() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    let pid = store
        .test_deposit_backend_pid(locks(Uuid::from_u128(0x7701)))
        .await
        .expect("事务内读取后端 pid");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let active: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE pid = $1")
                .bind(pid)
                .fetch_one(store.postgres_pool())
                .await
                .expect("查询 pg_stat_activity");
        if active == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "命令结束后专用连接必须关闭，pid {pid} 仍在 pg_stat_activity"
        );
        sleep(Duration::from_millis(50)).await;
    }
}
