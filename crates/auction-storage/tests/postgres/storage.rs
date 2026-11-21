//! PostgreSQL 18 集成测试骨架。
//!
//! 只有 `test-db` feature 才编译并运行本文件；`mise run verify` 的 all-targets 检查在这里
//! 只会看到一个空 crate，不连接数据库。入口见 `mise run test-db`。
//!
//! `support` 负责临时集群、模板库迁移与逐测试克隆库；测试本身只使用公开的存储入口。

use std::assert_matches;

use auction_application::auction::{
    PublicPriceHistoryListRequest, PublicPriceHistoryReader, PublicPriceHistorySort,
    PublicPriceHistoryStats, WorldReader,
};
use auction_application::core::{ApplicationError, SessionError};
use auction_application::public::{AdminSessionRecord, AdminSessionStore};
use auction_storage::postgres_expected_migration_version;
use auction_time::{Duration, Timestamp};

use crate::support;

/// 空库执行全部内嵌迁移后，通过世界目录端口读取迁移种子数据。
#[tokio::test]
async fn postgres_migrates_empty_database_and_reads_worlds() {
    let database = support::TestDatabase::create_empty().await.expect("创建空测试数据库");
    // 迁移须由 owner 执行；运行账号没有 DDL 权限。
    let store = database.maintenance_store().await.expect("连接空测试数据库");
    store.migrate().await.expect("空数据库执行全部迁移");

    let applied: i64 =
        sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM public._sqlx_migrations")
            .fetch_one(store.postgres_pool())
            .await
            .expect("读取已应用迁移版本");
    assert_eq!(
        applied,
        postgres_expected_migration_version(),
        "空库迁移后应达到二进制内嵌最高版本"
    );

    let worlds = store.list_worlds().await.expect("通过世界目录端口读取世界");
    let main = worlds.iter().find(|world| world.id == "main").expect("迁移应写入 main 世界");
    assert!(main.enabled, "main 世界默认启用");
    assert_eq!(main.availability, "available");
    assert!(main.policy_revision > 0);
    assert!(worlds.iter().any(|world| world.id == "isekai"), "迁移应写入 isekai 世界");

    // 097 删除无人读写的对象：空库迁移后不能残留视图、表或旧授权目标。
    for relation in [
        "bid_proxies",
        "bid_proxy_attempts",
        "bid_proxy_outbid_uses",
        "auction_forum_reply_cursors",
    ] {
        let present: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(format!("public.{relation}"))
            .fetch_one(store.postgres_pool())
            .await
            .expect("读取被删除对象的存在性");
        assert!(present.is_none(), "迁移 097 后 {relation} 不应存在");
    }
    let level: String = sqlx::query_scalar(
        "SELECT level FROM public.schema_migration_compatibility WHERE version = 97",
    )
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取迁移 097 兼容登记");
    assert_eq!(level, "breaking", "删除既有对象的迁移必须登记为 breaking");

    // 重复执行迁移必须是幂等的，不能再次改写已应用的历史。
    store.migrate().await.expect("重复执行迁移保持幂等");
    store.postgres_pool().close().await;
}

/// 每个测试从已迁移模板库克隆独立数据库，且克隆库不携带其他测试的数据。
#[tokio::test]
async fn postgres_clones_migrated_template_per_test() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");

    let applied: i64 =
        sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM public._sqlx_migrations")
            .fetch_one(store.postgres_pool())
            .await
            .expect("读取克隆库迁移版本");
    assert_eq!(applied, postgres_expected_migration_version());

    let auctions: i64 = sqlx::query_scalar("SELECT count(*) FROM public.auctions")
        .fetch_one(store.postgres_pool())
        .await
        .expect("读取克隆库场次行数");
    assert_eq!(auctions, 0, "克隆库只包含模板库的 schema 与迁移种子");

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_admin_session_inspect_leaves_activity_unchanged_and_touch_caps_at_absolute() {
    let database = support::TestDatabase::create().await.unwrap();
    let store = database.store().await.unwrap();
    let token_hash = [1; 32];
    let csrf_hash = [2; 32];
    let now = Timestamp::from_timestamp_micros(Timestamp::now().timestamp_micros()).unwrap();
    let at = |seconds| now.checked_add_signed(Duration::seconds(seconds)).unwrap();
    store
        .create_admin(AdminSessionRecord {
            token_hash,
            csrf_hash,
            actor: "operator".to_owned(),
            created_at: now,
            expires_at: at(900),
            absolute_expires_at: at(1_000),
        })
        .await
        .unwrap();
    let activity = || {
        sqlx::query_scalar::<_, Timestamp>(
            "SELECT last_activity_at FROM public.admin_sessions WHERE token_hash = $1",
        )
        .bind(token_hash.as_slice())
        .fetch_one(store.postgres_pool())
    };
    let checked = store.inspect_admin(token_hash, csrf_hash, at(200)).await.unwrap();
    assert_eq!(checked.expires_at, at(900));
    assert_eq!(activity().await.unwrap(), now);

    let renewed = store.touch_admin(token_hash, csrf_hash, at(200), at(1_100)).await.unwrap();
    assert_eq!(renewed.expires_at, at(1_000));
    assert_eq!(renewed.absolute_expires_at, at(1_000));
    assert_eq!(activity().await.unwrap(), at(200));
    let older = store.touch_admin(token_hash, csrf_hash, at(50), at(950)).await.unwrap();
    assert_eq!(older.expires_at, renewed.expires_at);
    assert_eq!(activity().await.unwrap(), at(200));

    for (csrf_hash, now) in [([3; 32], at(300)), (csrf_hash, at(1_000))] {
        let checked = store.inspect_admin(token_hash, csrf_hash, now).await;
        let touched = store.touch_admin(token_hash, csrf_hash, now, at(1_200)).await;
        assert_matches!(checked, Err(ApplicationError::Session(SessionError::Unauthenticated)));
        assert_matches!(touched, Err(ApplicationError::Session(SessionError::Unauthenticated)));
    }
    store.revoke_admin(token_hash, at(400)).await.unwrap();
    let checked = store.inspect_admin(token_hash, csrf_hash, at(450)).await;
    let touched = store.touch_admin(token_hash, csrf_hash, at(450), at(1_300)).await;
    assert_matches!(checked, Err(ApplicationError::Session(SessionError::Unauthenticated)));
    assert_matches!(touched, Err(ApplicationError::Session(SessionError::Unauthenticated)));
    assert_eq!(activity().await.unwrap(), at(200));
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_admin_session_cannot_be_validated_or_renewed_after_waiting_past_deadline() {
    let database = support::TestDatabase::create().await.unwrap();
    let store = database.store().await.unwrap();
    for renew in [false, true] {
        let token_hash = [u8::from(renew) + 1; 32];
        let csrf_hash = [3; 32];
        let now: Timestamp = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(store.postgres_pool())
            .await
            .expect("读取 PostgreSQL 时钟");
        let expires_at = now.checked_add_signed(Duration::seconds(2)).unwrap();
        let absolute_expires_at = now.checked_add_signed(Duration::minutes(5)).unwrap();
        store
            .create_admin(AdminSessionRecord {
                token_hash,
                csrf_hash,
                actor: "operator".to_owned(),
                created_at: now,
                expires_at,
                absolute_expires_at,
            })
            .await
            .unwrap();
        let mut transaction = store.postgres_pool().begin().await.unwrap();
        sqlx::query(
            "SELECT token_hash FROM public.admin_sessions WHERE token_hash = $1 FOR UPDATE",
        )
        .bind(token_hash.as_slice())
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
        let operation = async {
            if renew {
                store.touch_admin(token_hash, csrf_hash, now, absolute_expires_at).await
            } else {
                store.inspect_admin(token_hash, csrf_hash, now).await
            }
        };
        let release = async move {
            // 以数据库时间跨过截止，再释放另一连接上的会话行锁。
            sqlx::query("SELECT pg_sleep_until($1)")
                .bind(expires_at.checked_add_signed(Duration::milliseconds(10)).unwrap())
                .execute(&mut *transaction)
                .await
                .unwrap();
            transaction.commit().await.unwrap();
        };
        let (result, ()) = tokio::join!(operation, release);
        assert_matches!(result, Err(ApplicationError::Session(SessionError::Unauthenticated)));
    }
    store.postgres_pool().close().await;
}

/// 上限内的登录不注销任何已有会话。
#[tokio::test]
async fn postgres_public_login_keeps_sessions_below_the_limit() {
    use auction_application::public::{PublicSessionRecord, PublicSessionStore};

    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let user_id = seed_public_user(&store, "session-below-limit").await;
    let first = seed_public_session(&store, user_id, "below", 0, Timestamp::now()).await;
    let second = seed_public_session(&store, user_id, "below", 1, Timestamp::now()).await;

    let identity = store
        .create_public(
            PublicSessionRecord {
                token_hash: session_token_hash("below-new"),
                user_id,
                authenticated_user_version: 1,
                created_at: Timestamp::now(),
            },
            10,
        )
        .await
        .expect("上限内的 Public 登录应成功");
    assert_eq!(identity.user_id, user_id);

    let revoked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.public_sessions WHERE user_id = $1 AND revoked_at IS NOT NULL",
    )
    .bind(user_id)
    .fetch_one(store.postgres_pool())
    .await
    .expect("统计已注销会话");
    assert_eq!(revoked, 0, "未满额时不能注销既有会话");
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.public_sessions WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(store.postgres_pool())
    .await
    .expect("统计有效会话");
    assert_eq!(active, 3);
    assert!(first != second);

    store.postgres_pool().close().await;
}

/// 满额登录注销最久未活动的会话，保留记录并成功建立新会话；用户行锁内串行化。
#[tokio::test]
async fn postgres_public_login_evicts_least_recently_active_session_at_the_limit() {
    use auction_application::public::{PublicSessionRecord, PublicSessionStore};

    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let user_id = seed_public_user(&store, "session-at-limit").await;
    let base = Timestamp::now();

    // 创建顺序与活动顺序相反：最早创建的会话最近仍在活动，最晚创建的最久未活动。
    let mut tokens = Vec::new();
    for index in 0..10_u32 {
        let created_at = base - auction_time::Duration::minutes(i64::from(20 - index));
        let last_activity_at = base - auction_time::Duration::minutes(i64::from(index + 1));
        tokens.push(seed_public_session(&store, user_id, "limit", index, created_at).await);
        sqlx::query(
            "UPDATE public.public_sessions SET last_activity_at = $2 WHERE token_hash = $1",
        )
        .bind(session_token_hash(&format!("limit-{index}")))
        .bind(last_activity_at)
        .execute(store.postgres_pool())
        .await
        .expect("写入会话活动时间");
    }
    let stale_token = session_token_hash("limit-9");

    let identity = store
        .create_public(
            PublicSessionRecord {
                token_hash: session_token_hash("limit-new"),
                user_id,
                authenticated_user_version: 1,
                created_at: base,
            },
            10,
        )
        .await
        .expect("满额时登录应腾出名额而不是拒绝");
    assert_eq!(identity.user_id, user_id);

    let revoked: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT token_hash FROM public.public_sessions WHERE user_id = $1 AND revoked_at IS NOT NULL",
    )
    .bind(user_id)
    .fetch_all(store.postgres_pool())
    .await
    .expect("读取已注销会话");
    assert_eq!(revoked, vec![stale_token.to_vec()], "只注销最久未活动的一个会话");

    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.public_sessions WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(store.postgres_pool())
            .await
            .expect("统计会话总数");
    assert_eq!(total, 11, "注销只写 revoked_at，不删除历史记录");

    let stored_activity: auction_time::Timestamp = sqlx::query_scalar(
        "SELECT last_activity_at FROM public.public_sessions WHERE token_hash = $1",
    )
    .bind(stale_token.as_slice())
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取被注销会话的活动时间");
    assert!(
        stored_activity < base - auction_time::Duration::minutes(5),
        "被注销会话的活动时间不被改写"
    );

    // 再登录一次继续按同一顺序腾位，而不是只认最早创建的行。
    store
        .create_public(
            PublicSessionRecord {
                token_hash: session_token_hash("limit-newer"),
                user_id,
                authenticated_user_version: 1,
                created_at: base + auction_time::Duration::minutes(1),
            },
            10,
        )
        .await
        .expect("后续满额登录仍应成功");
    let revoked_second: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT token_hash FROM public.public_sessions WHERE user_id = $1 AND revoked_at IS NOT NULL ORDER BY revoked_at, token_hash",
    )
    .bind(user_id)
    .fetch_all(store.postgres_pool())
    .await
    .expect("读取第二次注销结果");
    assert_eq!(revoked_second.len(), 2);
    assert!(revoked_second.contains(&tokens[8].clone()), "第二旧的活动会话应被注销");

    store.postgres_pool().close().await;
}

fn session_token_hash(seed: &str) -> [u8; 32] {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(seed.as_bytes());
    let mut hash = [0_u8; 32];
    hash.copy_from_slice(&digest);
    hash
}

async fn seed_public_user(store: &auction_storage::DatabaseStore, seed: &str) -> uuid::Uuid {
    let user_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, seed.as_bytes());
    sqlx::query(
        "INSERT INTO public.users (id, username, public_key_hash)
         VALUES ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(seed)
    .bind(session_token_hash(&format!("{seed}-key")).as_slice())
    .execute(store.postgres_pool())
    .await
    .unwrap_or_else(|error| panic!("写入测试用户失败：{error}"));
    user_id
}

async fn seed_public_session(
    store: &auction_storage::DatabaseStore,
    user_id: uuid::Uuid,
    seed: &str,
    index: u32,
    created_at: auction_time::Timestamp,
) -> Vec<u8> {
    let token_hash = session_token_hash(&format!("{seed}-{index}"));
    sqlx::query(
        "INSERT INTO public.public_sessions
            (token_hash, user_id, created_at, last_activity_at, expires_at, absolute_expires_at)
         VALUES ($1, $2, $3, $3, NULL, NULL)",
    )
    .bind(token_hash.as_slice())
    .bind(user_id)
    .bind(created_at)
    .execute(store.postgres_pool())
    .await
    .unwrap_or_else(|error| panic!("写入测试会话失败：{error}"));
    token_hash.to_vec()
}

/// 验收：以运行账号执行 rekey 后，旧 key 无法登录、原有会话全部失效、新 key 可登录；
/// 同一 command_id 重放返回原结果（不变量 1），不再改写 key 或会话。
#[tokio::test]
async fn postgres_rekey_revokes_old_key_and_sessions_as_runtime_account() {
    use auction_application::core::{ApplicationError, RequestHash, SessionError};
    use auction_application::mail::{
        MailLoginKeyEnvelope, MailUserRekeyCommand, MailUserRekeyWrite, MailUserResultCode,
        MailUserStore,
    };
    use auction_application::public::{PublicLoginAuthenticator, PublicSessionStore};
    use auction_domain::CommandId;

    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    // seed_public_user 以 sha256("<seed>-key") 作为登录 key 摘要，原文即旧 key。
    let user_id = seed_public_user(&store, "rekey-user").await;
    let old_key = b"rekey-user-key".to_vec();
    let new_key = b"rekey-user-new-key".to_vec();
    let mut sessions = Vec::new();
    for index in 0..2 {
        let token = seed_public_session(&store, user_id, "rekey", index, Timestamp::now()).await;
        sessions.push(<[u8; 32]>::try_from(token.as_slice()).expect("32 字节会话摘要"));
    }
    assert_eq!(
        store.authenticate(old_key.clone()).await.expect("轮换前旧 key 可登录").user_id,
        user_id
    );
    for token in &sessions {
        store.inspect_public(*token, Timestamp::now()).await.expect("轮换前会话有效");
    }

    let command_id = CommandId::new();
    let now = Timestamp::now();
    let write = || MailUserRekeyWrite {
        command: MailUserRekeyCommand {
            command_id,
            request_hash: RequestHash::new([7; 32]),
            username: "rekey-user".to_owned(),
            now,
        },
        user_id,
        public_key_hash: session_token_hash("rekey-user-new-key"),
        envelope: MailLoginKeyEnvelope::new(
            vec![1; 16],
            vec![2; 12],
            "test-key".to_owned(),
            "test-fingerprint".to_owned(),
        )
        .expect("测试信封"),
    };
    let result = store.rekey_login_key(write()).await.expect("轮换成功");
    assert_eq!(result.code, MailUserResultCode::Rekeyed);
    assert_eq!(result.user_id, Some(user_id));

    let rejected = store.authenticate(old_key.clone()).await.expect_err("旧 key 不能再登录");
    assert_matches!(rejected, ApplicationError::Session(SessionError::InvalidCredentials));
    assert_eq!(store.authenticate(new_key.clone()).await.expect("新 key 可登录").user_id, user_id);
    for token in sessions {
        assert!(store.inspect_public(token, Timestamp::now()).await.is_err(), "旧会话必须全部失效");
    }

    // 同一命令重放只返回原结果：新 key 仍然有效，旧 key 仍被拒绝。
    let replay = store.rekey_login_key(write()).await.expect("重放返回原结果");
    assert_eq!(replay.code, MailUserResultCode::Rekeyed);
    assert!(store.authenticate(old_key).await.is_err());
    assert_eq!(store.authenticate(new_key).await.expect("重放后新 key 仍可登录").user_id, user_id);

    store.postgres_pool().close().await;
}

/// 同账号并发满额登录：用户行锁串行化，两个新会话都成功且有效会话不超上限。
#[tokio::test]
async fn postgres_public_login_serializes_concurrent_evictions_under_user_lock() {
    use auction_application::public::{PublicSessionRecord, PublicSessionStore};

    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let user_id = seed_public_user(&store, "session-concurrent").await;
    let base = Timestamp::now();
    for index in 0..10_u32 {
        seed_public_session(&store, user_id, "concurrent", index, base).await;
    }

    let mut blocker = store.postgres_pool().begin().await.expect("开启用户锁事务");
    sqlx::query("SELECT id FROM public.users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .fetch_one(&mut *blocker)
        .await
        .expect("持有用户行锁");
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .expect("读取锁持有会话");
    let first_store = store.clone();
    let first = tokio::spawn(async move {
        first_store
            .create_public(
                PublicSessionRecord {
                    token_hash: session_token_hash("concurrent-a"),
                    user_id,
                    authenticated_user_version: 1,
                    created_at: Timestamp::now(),
                },
                10,
            )
            .await
    });
    let second_store = store.clone();
    let second = tokio::spawn(async move {
        second_store
            .create_public(
                PublicSessionRecord {
                    token_hash: session_token_hash("concurrent-b"),
                    user_id,
                    authenticated_user_version: 1,
                    created_at: Timestamp::now(),
                },
                10,
            )
            .await
    });
    let mut both_waiting = false;
    for _ in 0..400 {
        let waiting = support::lock_waiters::count_blocked_by(store.postgres_pool(), blocker_pid)
            .await
            .expect("读取用户行锁等待");
        if waiting >= 2 {
            both_waiting = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(both_waiting, "两个登录都应排队等待同一用户行锁");
    blocker.commit().await.expect("释放用户行锁");
    first.await.expect("并发登录 A 任务结束").expect("并发登录 A 应成功");
    second.await.expect("并发登录 B 任务结束").expect("并发登录 B 应成功");

    let (active, revoked): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE revoked_at IS NULL),
                count(*) FILTER (WHERE revoked_at IS NOT NULL)
         FROM public.public_sessions WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(store.postgres_pool())
    .await
    .expect("统计并发登录后的会话");
    assert_eq!(active, 10, "并发登录后有效会话仍等于上限");
    assert_eq!(revoked, 2, "两次满额登录各注销一个最久未活动的会话");

    store.postgres_pool().close().await;
}

/// 迁移 000084 的价格历史索引必须与查询里恒定的可见集合条件一致：只覆盖已成交且有买家的
/// lot，前导键是游标页的排序键。索引是纯候选入口，可见集合仍由 SQL 决定。
#[tokio::test]
async fn postgres_price_history_index_matches_owner_predicate() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    use sqlx::Row;
    use uuid::Uuid;

    let pool = store.postgres_pool();
    let request = PublicPriceHistoryListRequest {
        name_terms: Vec::new(),
        world_id: None,
        closed_from: None,
        closed_before: None,
        minimum_price: None,
        maximum_price: None,
        buyer: None,
        seller: None,
        sort: PublicPriceHistorySort::ClosedDesc,
        limit: 25,
        offset: 0,
        cursor: None,
    };
    let empty_page =
        store.list_public_price_history(request.clone()).await.expect("空库价格历史编号页");
    assert!(empty_page.entries.is_empty());
    assert_eq!(empty_page.total, Some(0));
    assert_eq!(
        empty_page.stats,
        Some(PublicPriceHistoryStats {
            count: 0,
            minimum: 0,
            maximum: 0,
            median: 0,
            median_exact: None,
        }),
    );

    let now = Timestamp::now();
    let closed_at = now - Duration::hours(1);
    let auction_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let buyer_id = Uuid::new_v4();
    let sold_lot_id = Uuid::new_v4();
    let unsold_lot_id = Uuid::new_v4();
    let sold_equipment_id = Uuid::new_v4();
    let unsold_equipment_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.users (id, username, public_key_hash)
         VALUES ($1, 'price-history-buyer', decode(repeat('ab', 32), 'hex'))",
    )
    .bind(buyer_id)
    .execute(pool)
    .await
    .expect("写入价格历史买家");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, settled_at, scheduled_open_at,
             preview_scheduled_at, scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1, 'main', 900001, 'price history index fixture', 'settled', $2,
                 $3, $3, $3, $2, $4)",
    )
    .bind(auction_id)
    .bind(closed_at)
    .bind(closed_at - Duration::days(1))
    .bind(serde_json::json!({
        "schema_version": 4,
        "start_price": 100,
        "bid_increment_ppm": 0,
        "minimum_increment": 1,
        "soft_close_window_seconds": 900,
        "soft_close_extension_seconds": 900,
        "proxy_enabled": true,
        "proxy_default_mode": "fast",
        "proxy_slow_interval_seconds": 600,
        "proxy_max_outbid_uses": 0,
        "final_phase_lead_seconds": 900,
        "scheduled_end_at": closed_at,
        "settlement": {"version": 0, "mode": "legacy_zero_v0", "anchors": []},
        "recovery": {"version": 1, "mode": "remind_v1", "payment_timeout_seconds": 604800}
    }))
    .execute(pool)
    .await
    .expect("写入价格历史场次");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'settle_auction', $2, decode(repeat('cd', 32), 'hex'), 'accepted')",
    )
    .bind(command_id)
    .bind(auction_id)
    .execute(pool)
    .await
    .expect("写入价格历史结算命令");
    sqlx::query(
        "INSERT INTO public.auction_settlements
            (auction_id, command_id, policy_version, policy_mode, pool_enabled,
             opening_pool_balance, closing_pool_balance, settled_at)
         VALUES ($1, $2, 0, 'legacy_zero_v0', false, 0, 0, $3)",
    )
    .bind(auction_id)
    .bind(command_id)
    .bind(closed_at)
    .execute(pool)
    .await
    .expect("写入价格历史场次结算");
    for (id, external_id, name) in [
        (sold_equipment_id, "price-history-sold", "Sold history staff"),
        (unsold_equipment_id, "price-history-unsold", "Unsold history staff"),
    ] {
        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             VALUES ($1, 'main', $2, $3, 'Staff', '{}'::jsonb, $4)",
        )
        .bind(id)
        .bind(external_id)
        .bind(name)
        .bind(closed_at)
        .execute(pool)
        .await
        .expect("写入价格历史装备");
    }
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             leader_user_id, effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, 1, 'sold', 100, 500, $4, $5, 'Staff', 1),
                ($6, $2, $7, 2, 'unsold', 100, NULL, NULL, $5, 'Staff', 2)",
    )
    .bind(sold_lot_id)
    .bind(auction_id)
    .bind(sold_equipment_id)
    .bind(buyer_id)
    .bind(closed_at)
    .bind(unsold_lot_id)
    .bind(unsold_equipment_id)
    .execute(pool)
    .await
    .expect("写入价格历史成交与流拍 lot");
    sqlx::query(
        "INSERT INTO public.lot_settlements
            (lot_id, end_price, listed_price, buyer_amount, seller_amount, tax_amount,
             compensation_amount, hold_ppm, adjustment_ppm)
         VALUES ($1, 500, 500, 500, 500, 0, 0, 0, 0)",
    )
    .bind(sold_lot_id)
    .execute(pool)
    .await
    .expect("写入价格历史 lot 结算");

    let (definition, predicate): (String, Option<String>) = sqlx::query_as(
        "SELECT pg_get_indexdef(i.indexrelid), pg_get_expr(i.indpred, i.indrelid)
         FROM pg_index i
         JOIN pg_class c ON c.oid = i.indexrelid
         WHERE c.relname = 'lots_sold_history_effective_end_idx'",
    )
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取价格历史索引定义");

    assert!(
        definition.contains("btree (effective_end_at DESC, id)"),
        "前导键必须是游标页排序键：{definition}"
    );
    assert!(
        definition.contains("INCLUDE (auction_id, current_price, lot_number)"),
        "非搜索列以 INCLUDE 提供，避免改变唯一性或写入要求：{definition}"
    );
    let predicate = predicate.expect("部分索引必须带谓词");
    assert!(
        predicate.contains("status = 'sold'::text")
            && predicate.contains("leader_user_id IS NOT NULL"),
        "谓词必须与查询恒定条件一致：{predicate}"
    );

    // 比较真实非空成交集合：已成交 lot 可见，流拍 lot 不可见；关闭所有索引扫描后集合应相同。
    let page = store.list_public_price_history(request.clone()).await.expect("价格历史编号页");
    let indexed_ids = page.entries.iter().map(|entry| entry.lot_id).collect::<Vec<_>>();
    assert_eq!(indexed_ids, vec![sold_lot_id], "应显示成交记录并排除流拍 lot");
    assert_eq!(page.total, Some(1));
    assert_eq!(
        page.stats,
        Some(PublicPriceHistoryStats {
            count: 1,
            minimum: 500,
            maximum: 500,
            median: 500,
            median_exact: Some("500".to_owned()),
        }),
    );

    let filtered = store
        .list_public_price_history(PublicPriceHistoryListRequest {
            minimum_price: Some(501),
            ..request.clone()
        })
        .await
        .expect("无匹配成交记录的价格历史编号页");
    assert_eq!(filtered, empty_page);

    let past_end = store
        .list_public_price_history(PublicPriceHistoryListRequest {
            offset: request.limit,
            ..request
        })
        .await
        .expect("超出成交记录范围的价格历史编号页");
    assert!(past_end.entries.is_empty());
    assert_eq!(past_end.total, page.total);
    assert_eq!(past_end.stats, page.stats);

    let mut transaction = pool.begin().await.expect("开启无索引扫描对照事务");
    sqlx::query("SET LOCAL enable_indexscan = off")
        .execute(&mut *transaction)
        .await
        .expect("关闭 index scan");
    sqlx::query("SET LOCAL enable_indexonlyscan = off")
        .execute(&mut *transaction)
        .await
        .expect("关闭 index-only scan");
    sqlx::query("SET LOCAL enable_bitmapscan = off")
        .execute(&mut *transaction)
        .await
        .expect("关闭 bitmap scan");
    let rows = sqlx::query(auction_storage::benchmark_sql::numbered_query_sql())
        .bind(None::<Vec<String>>)
        .bind(None::<&str>)
        .bind(None::<Timestamp>)
        .bind(None::<Timestamp>)
        .bind(None::<i64>)
        .bind(None::<i64>)
        .bind(None::<&str>)
        .bind(None::<&str>)
        .bind(25_i64)
        .bind(0_i64)
        .bind("closed_desc")
        .fetch_all(&mut *transaction)
        .await
        .expect("无索引扫描价格历史查询");
    let sequential_ids = rows
        .iter()
        .filter_map(|row| row.try_get::<Option<Uuid>, _>("lot_id").expect("读取 lot id"))
        .collect::<Vec<_>>();
    assert_eq!(sequential_ids, indexed_ids, "索引开关不得改变可见历史集合");
    transaction.commit().await.expect("提交无索引扫描对照事务");

    store.postgres_pool().close().await;
}

/// readiness 必须区分“真实依赖故障”和暂时饱和：池关闭后不得仍报告就绪。
#[tokio::test]
async fn postgres_readiness_reports_real_database_failure() {
    use auction_application::operations::ProcessReadinessPort;

    let database = support::TestDatabase::create().await.expect("克隆已迁移模板库");
    let store = database.store().await.expect("连接测试数据库");
    store.check().await.expect("刚连接的已迁移数据库应就绪");
    store.postgres_pool().close().await;
    assert!(store.check().await.is_err(), "连接池关闭后不得报告就绪");
}
