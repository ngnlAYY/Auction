//! PostgreSQL 18 集成测试骨架（W6.2）。
//!
//! 只有 `test-db` feature 才编译并运行本文件；`just verify` 的 all-targets 检查在这里
//! 只会看到一个空 crate，不连接数据库。入口见 `just test-db`。
//!
//! `support` 负责临时集群、模板库迁移与逐测试克隆库；测试本身只使用公开的存储入口。
#![cfg(feature = "test-db")]

use crate::support;

use auction_application::auction::{PublicPriceHistoryReader, WorldReader};
use auction_storage::postgres_expected_migration_version;
use chrono::Utc;

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

/// 上限内的登录不注销任何已有会话。
#[tokio::test]
async fn postgres_public_login_keeps_sessions_below_the_limit() {
    use auction_application::public::{PublicSessionRecord, PublicSessionStore};

    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let user_id = seed_public_user(&store, "session-below-limit").await;
    let first = seed_public_session(&store, user_id, "below", 0, Utc::now()).await;
    let second = seed_public_session(&store, user_id, "below", 1, Utc::now()).await;

    let identity = store
        .create_public(
            PublicSessionRecord {
                token_hash: session_token_hash("below-new"),
                user_id,
                authenticated_user_version: 1,
                created_at: Utc::now(),
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
    let base = Utc::now();

    // 创建顺序与活动顺序相反：最早创建的会话最近仍在活动，最晚创建的最久未活动。
    let mut tokens = Vec::new();
    for index in 0..10_u32 {
        let created_at = base - chrono::Duration::minutes(i64::from(20 - index));
        let last_activity_at = base - chrono::Duration::minutes(i64::from(index + 1));
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

    let stored_activity: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT last_activity_at FROM public.public_sessions WHERE token_hash = $1",
    )
    .bind(stale_token.as_slice())
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取被注销会话的活动时间");
    assert!(stored_activity < base - chrono::Duration::minutes(5), "被注销会话的活动时间不被改写");

    // 再登录一次继续按同一顺序腾位，而不是只认最早创建的行。
    store
        .create_public(
            PublicSessionRecord {
                token_hash: session_token_hash("limit-newer"),
                user_id,
                authenticated_user_version: 1,
                created_at: base + chrono::Duration::minutes(1),
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
    assert!(revoked_second.contains(&tokens[8].to_vec()), "第二旧的活动会话应被注销");

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
    created_at: chrono::DateTime<Utc>,
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

/// W9.10 验收：以运行账号执行 rekey 后，旧 key 无法登录、原有会话全部失效、新 key 可登录；
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
        let token = seed_public_session(&store, user_id, "rekey", index, Utc::now()).await;
        sessions.push(<[u8; 32]>::try_from(token.as_slice()).expect("32 字节会话摘要"));
    }
    assert_eq!(
        store.authenticate(old_key.clone()).await.expect("轮换前旧 key 可登录").user_id,
        user_id
    );
    for token in &sessions {
        store.inspect_public(*token, Utc::now()).await.expect("轮换前会话有效");
    }

    let command_id = CommandId::new();
    let now = Utc::now();
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
    assert!(matches!(rejected, ApplicationError::Session(SessionError::InvalidCredentials)));
    assert_eq!(store.authenticate(new_key.clone()).await.expect("新 key 可登录").user_id, user_id);
    for token in sessions {
        assert!(store.inspect_public(token, Utc::now()).await.is_err(), "旧会话必须全部失效");
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
    let base = Utc::now();
    for index in 0..10_u32 {
        seed_public_session(&store, user_id, "concurrent", index, base).await;
    }

    let login = |seed: &'static str| {
        let store = &store;
        async move {
            store
                .create_public(
                    PublicSessionRecord {
                        token_hash: session_token_hash(seed),
                        user_id,
                        authenticated_user_version: 1,
                        created_at: Utc::now(),
                    },
                    10,
                )
                .await
        }
    };
    let (first, second) = tokio::join!(login("concurrent-a"), login("concurrent-b"));
    first.expect("并发登录 A 应成功");
    second.expect("并发登录 B 应成功");

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

    // 索引不得改变价格历史的可见集合：空库上带与不带索引的结果都必须为空页。
    let request = auction_application::auction::PublicPriceHistoryListRequest {
        name_terms: Vec::new(),
        world_id: None,
        closed_from: None,
        closed_before: None,
        minimum_price: None,
        maximum_price: None,
        buyer: None,
        seller: None,
        sort: auction_application::auction::PublicPriceHistorySort::ClosedDesc,
        limit: 25,
        offset: 0,
        cursor: None,
    };
    let page = store.list_public_price_history(request).await.expect("空库价格历史编号页");
    assert_eq!(page.entries.len(), 0, "空库不应有历史行");
    assert_eq!(page.total, Some(0), "空库统计应为零");

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
