//! 4B 预约/代管端口测试的动态时钟夹具。

use std::sync::Arc;

use auction_application::consignment::{
    DepositCommandService, ExpireReservationCommand, IssueReservationCommand,
    ReservationInvalidation, ReservationSecretAad, ReservationSecretCrypto,
    ReservationSecretCryptoError, SealedReservationSecret,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::deposit::{TokenText, format_token, parse_token};
use auction_domain::consignment::{FeeBps, ListingPrice, ListingVersion};
use auction_domain::{CommandId, SettlementWorld, UserId};
use auction_storage::{DepositRepository, DepositSettings};
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use crate::support::TestDatabase;

pub const USER: &str = "10000000-0000-0000-0000-000000000001";
pub const USER_NAME: &str = "FakeSellerOne";
pub const LISTING: &str = "20000000-0000-0000-0000-0000000000b1";
pub const REPLAY: &str = "90000000-0000-0000-0000-0000000000b1";
pub const REGISTER: &str = "40000000-0000-4000-8000-0000000000b1";
pub const PUBLISH: &str = "41000000-0000-5000-8000-0000000000b1";
pub const MAIN_CREDENTIAL: &str = "80000000-0000-0000-0000-0000000000b1";
pub const MAIN_SESSION: &str = "81000000-0000-0000-0000-0000000000b1";
pub const ISEKAI_CREDENTIAL: &str = "80000000-0000-0000-0000-0000000000b2";
pub const ISEKAI_SESSION: &str = "81000000-0000-0000-0000-0000000000b2";

/// 测试 AEAD：密文只在进程内保存规范文本，用来证明端口与真实数据库的接线。
pub struct TestCrypto;

impl ReservationSecretCrypto for TestCrypto {
    fn seal(
        &self,
        token: &TokenText,
        _aad: ReservationSecretAad,
    ) -> Result<SealedReservationSecret, ReservationSecretCryptoError> {
        SealedReservationSecret::new(
            token.as_str().as_bytes().to_vec(),
            [7_u8; 12],
            "test-key".to_owned(),
            "a".repeat(64),
        )
    }

    fn open(
        &self,
        sealed: &SealedReservationSecret,
        _aad: ReservationSecretAad,
    ) -> Result<TokenText, ReservationSecretCryptoError> {
        let text =
            std::str::from_utf8(sealed.ciphertext()).map_err(|_| ReservationSecretCryptoError)?;
        let token = parse_token(text).map_err(|_| ReservationSecretCryptoError)?;
        Ok(format_token(*token.as_bytes()))
    }
}

/// 以运行账号构造真实存储端口与测试密码边界。
pub async fn repository(database: &TestDatabase) -> DepositRepository {
    repository_with_max(database, 100).await
}

/// 以指定活动额度构造真实存储端口，用于取得时的额度阻断。
pub async fn repository_with_max(
    database: &TestDatabase,
    max_open_per_user: i64,
) -> DepositRepository {
    repository_with_settings(
        database,
        DepositSettings::new(
            FeeBps::new(500).expect("测试费率"),
            ListingPrice::new(ListingPrice::MIN).expect("测试下限"),
            ListingPrice::new(ListingPrice::MAX).expect("测试上限"),
            24,
            max_open_per_user,
        ),
    )
    .await
}

/// 以完整启动配置构造真实存储端口。
pub async fn repository_with_settings(
    database: &TestDatabase,
    settings: DepositSettings,
) -> DepositRepository {
    DepositRepository::new(
        database.store().await.expect("运行账号"),
        Arc::new(TestCrypto),
        settings,
    )
}

/// 以同一真实存储组合应用层收件命令服务，用于软关闭后的只读重放。
pub fn command_service(repository: &DepositRepository) -> DepositCommandService {
    DepositCommandService::new(
        Arc::new(repository.clone()),
        Arc::new(repository.clone()),
        Arc::new(repository.clone()),
        Arc::new(repository.clone()),
        Arc::new(repository.clone()),
    )
}

/// 固定的请求摘要，测试内不参与业务判断。
pub const fn hash(seed: u8) -> RequestHash {
    RequestHash::new([seed; 32])
}

/// 固定的玩家 UUIDv4 命令。
pub fn client_command(seed: u128) -> CommandId {
    CommandId::from_uuid(Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 + seed))
}

/// 固定的 Worker UUIDv5 外观命令（只用于测试账本形状）。
pub fn worker_command(seed: u128) -> CommandId {
    CommandId::from_uuid(Uuid::from_u128(0x0000_0000_0000_5000_8000_0000_0000_0000 + seed))
}

/// 插入用户、登记重放钥与 main 的未来待寄挂单。
pub async fn seed_listing(pool: &PgPool, listing_id: Uuid, world: SettlementWorld) -> String {
    seed_listing_window(pool, listing_id, world, None, None, "consigned", "awaiting_deposit", "42")
        .await
}

/// 插入带指定登记时刻与待寄截止的挂单，用于季末边界与截止钳制。
pub async fn seed_listing_at(
    pool: &PgPool,
    listing_id: Uuid,
    world: SettlementWorld,
    created_at: auction_time::Timestamp,
    expires_at: auction_time::Timestamp,
) -> String {
    seed_listing_window(
        pool,
        listing_id,
        world,
        Some(created_at),
        Some(expires_at),
        "consigned",
        "awaiting_deposit",
        "42",
    )
    .await
}

/// 插入带指定登记时刻和 EID 的待寄挂单。
pub async fn seed_listing_eid_at(
    pool: &PgPool,
    listing_id: Uuid,
    world: SettlementWorld,
    created_at: auction_time::Timestamp,
    expires_at: auction_time::Timestamp,
    eid: &str,
) -> String {
    seed_listing_window(
        pool,
        listing_id,
        world,
        Some(created_at),
        Some(expires_at),
        "consigned",
        "awaiting_deposit",
        eid,
    )
    .await
}

/// 插入带指定登记时刻和 EID 的 `direct/displayed` 挂单。
pub async fn seed_displayed_listing_eid_at(
    pool: &PgPool,
    listing_id: Uuid,
    world: SettlementWorld,
    created_at: auction_time::Timestamp,
    expires_at: auction_time::Timestamp,
    eid: &str,
) -> String {
    seed_listing_window(
        pool,
        listing_id,
        world,
        Some(created_at),
        Some(expires_at),
        "direct",
        "displayed",
        eid,
    )
    .await
}

#[allow(clippy::too_many_arguments, reason = "测试夹具逐项复刻迁移列")]
async fn seed_listing_window(
    pool: &PgPool,
    listing_id: Uuid,
    world: SettlementWorld,
    created_at: Option<auction_time::Timestamp>,
    expires_at: Option<auction_time::Timestamp>,
    mode: &str,
    published: &str,
    eid: &str,
) -> String {
    let world_code = auction_domain::consignment::settlement_world_code(world);
    let listing = listing_id.to_string();
    let snapshot = Uuid::new_v4().to_string();
    let created_sql = created_at.map_or_else(
        || "transaction_timestamp()".to_owned(),
        |value| format!("timestamptz '{value}'"),
    );
    let expires_sql = expires_at.map_or_else(
        || "timestamptz '2099-01-01T00:00:00Z'".to_owned(),
        |value| format!("timestamptz '{value}'"),
    );
    let expires_text =
        expires_at.map_or_else(|| "2099-01-01T00:00:00Z".into(), |value| format!("{value:?}"));
    let fee_bps = if mode == "direct" { 0 } else { 500 };
    let register_id = Uuid::new_v4();
    let publish_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, listing_id.as_bytes());
    let sql = format!(
        "INSERT INTO public.users (id, username, public_key_hash) VALUES
            ('{USER}', '{USER_NAME}', decode(repeat('11', 32), 'hex'))
         ON CONFLICT (id) DO NOTHING;
         INSERT INTO public.consignment_replay_keys
            (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
         VALUES ('{REPLAY}', true, decode(repeat('13', 48), 'hex'),
                 decode(repeat('14', 12), 'hex'), 'fake-data-key',
                 transaction_timestamp(), transaction_timestamp())
         ON CONFLICT (id) DO NOTHING;
         INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
             blacklist_baseline_ids, created_at, updated_at, expires_at)
         VALUES ('{listing}', '{USER}', '{world_code}', '{eid}', '{mode}', 'resolving', 101,
                 1, {fee_bps}, 1, 1, 0, ARRAY[]::uuid[], {created_sql}, transaction_timestamp(),
                 {expires_sql});
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{REGISTER}', 'public_user', '{USER}', 'register', 'new',
                 decode(repeat('15', 32), 'hex'), '{REPLAY}', '{listing}', 202, 'accepted',
                 '{{\"command_id\":\"{REGISTER}\",\"operation\":\"register\",
                    \"listing_id\":\"{listing}\",\"listing_version\":1,
                    \"state\":\"resolving\",\"disposition\":\"registered\",\"price\":101}}',
                 transaction_timestamp());
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{listing}', 1, '{REGISTER}', 'public_user', 'registered', NULL,
                 'resolving', NULL, 101, transaction_timestamp());
         INSERT INTO public.consignment_snapshots
            (id, listing_id, generation, adapter, observed_at, digest, world_id,
             external_equipment_id, name, category, alt_name, level_kind, level_number,
             tradeable, owner_name, stat_groups)
         VALUES ('{snapshot}', '{listing}', 1, 'hvdata-equip-v1', transaction_timestamp(),
                 decode(repeat('16', 32), 'hex'), '{world_code}', '{eid}', 'Fake Gear', 'Weapon',
                 NULL, 'assigned', 100, true, '{USER_NAME}', '[]'::jsonb);
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{PUBLISH}', 'worker', 'fake-worker', 'apply_lookup', '{listing}',
                 decode(repeat('17', 32), 'hex'), NULL, '{listing}', 200, 'ok',
                 '{{\"command_id\":\"{PUBLISH}\",\"operation\":\"apply_lookup\",
                    \"listing_id\":\"{listing}\",\"listing_version\":2,
                    \"state\":\"{published}\",\"disposition\":\"published\",\"price\":101,
                    \"expires_at\":\"{expires_text}\",\"reason_code\":null}}',
                 transaction_timestamp());
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{listing}', 2, '{PUBLISH}', 'worker', 'published', 'resolving',
                 '{published}', NULL, 101, transaction_timestamp());
         UPDATE public.consignment_listings
         SET state='{published}', state_version=2, current_snapshot_id='{snapshot}',
             updated_at=transaction_timestamp(), expires_at={expires_sql}
         WHERE id='{listing}';"
    )
    .replace(REGISTER, &register_id.to_string())
    .replace(PUBLISH, &publish_id.to_string());
    let mut transaction = pool.begin().await.expect("开启挂单夹具事务");
    sqlx::raw_sql(AssertSqlSafe(sql)).execute(&mut *transaction).await.expect("写入未来待寄挂单");
    transaction.commit().await.expect("提交挂单夹具");
    snapshot
}

/// 插入 main 与 isekai 的收件就绪行，并打开寄售解析闸门。
pub async fn seed_mail_ready(pool: &PgPool) {
    let sql = format!(
        "INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
             fingerprint, status, state_version, created_at, updated_at)
         VALUES ('{MAIN_CREDENTIAL}', 'mail_cookie', 'main', 'fake', decode('11', 'hex'),
                 decode(repeat('22', 12), 'hex'), 'fake-data-key', repeat('a', 64),
                 'active', 1, clock_timestamp(), clock_timestamp())
         ON CONFLICT DO NOTHING;
         INSERT INTO public.mail_evidence_sessions
            (id, bot_uid, world_id, player_name, credential_id, credential_version,
             runtime_generation, observed_at, protocol_version, binding_digest)
         VALUES ('{MAIN_SESSION}', '12345', 'main', 'FakeBot', '{MAIN_CREDENTIAL}', 1, 1,
                 clock_timestamp(), 'v1', decode(repeat('33', 32), 'hex'));
         INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('main', '{MAIN_SESSION}', 1, decode(repeat('35', 32), 'hex'), 'enforced',
                 ARRAY['{USER_NAME}'], ARRAY[]::text[], true, true, true,
                 clock_timestamp(), clock_timestamp() + interval '1 day', 1);
         UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1;"
    );
    let mut transaction = pool.begin().await.expect("开启收件夹具事务");
    sqlx::raw_sql(AssertSqlSafe(sql)).execute(&mut *transaction).await.expect("写入收件就绪");
    transaction.commit().await.expect("提交收件夹具");
}

/// 单独为 isekai 测试插入该世界唯一的 active mail cookie 与就绪行。
pub async fn seed_isekai_ready(pool: &PgPool) {
    let sql = format!(
        "INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
             fingerprint, status, state_version, created_at, updated_at)
         VALUES ('{ISEKAI_CREDENTIAL}', 'mail_cookie', 'isekai', 'fake', decode('12', 'hex'),
                 decode(repeat('23', 12), 'hex'), 'fake-data-key', repeat('b', 64),
                 'active', 1, clock_timestamp(), clock_timestamp());
         INSERT INTO public.mail_evidence_sessions
            (id, bot_uid, world_id, player_name, credential_id, credential_version,
             runtime_generation, observed_at, protocol_version, binding_digest)
         VALUES ('{ISEKAI_SESSION}', '12346', 'isekai', 'FakeBotIsekai', '{ISEKAI_CREDENTIAL}',
                 1, 1, clock_timestamp(), 'v1', decode(repeat('34', 32), 'hex'));
         INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('isekai', '{ISEKAI_SESSION}', 1, decode(repeat('36', 32), 'hex'), 'enforced',
                 ARRAY['{USER_NAME}'], ARRAY[]::text[], true, true, true,
                 clock_timestamp(), clock_timestamp() + interval '1 day', 1);
         UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1;"
    );
    let mut transaction = pool.begin().await.expect("开启 isekai 收件夹具事务");
    sqlx::raw_sql(AssertSqlSafe(sql))
        .execute(&mut *transaction)
        .await
        .expect("写入 isekai 收件就绪");
    transaction.commit().await.expect("提交 isekai 收件夹具");
}

/// 主世界 issue 命令。
pub fn issue_command(
    command_id: CommandId,
    listing_id: Uuid,
    expected_version: u64,
    seed: u8,
) -> IssueReservationCommand {
    IssueReservationCommand {
        command_id,
        seller_user_id: UserId::from_uuid(Uuid::parse_str(USER).expect("固定用户 UUID")),
        listing_id,
        expected_version: ListingVersion::new(expected_version).expect("测试版本"),
        request_hash: hash(seed),
        request_id: None,
    }
}

/// 过期退休命令。
pub fn expire_command(
    command_id: CommandId,
    reservation_id: Uuid,
    generation: u64,
    expires_at: auction_time::Timestamp,
) -> ExpireReservationCommand {
    ExpireReservationCommand {
        command_id,
        reservation_id,
        generation,
        invalidation: ReservationInvalidation::Expired { expires_at },
        request_hash: hash(0x74),
    }
}

/// 直接插入一条已到期但仍 active 的预约与强占用，用于退休/过期测试。
pub async fn seed_active_reservation(
    pool: &PgPool,
    listing_id: Uuid,
    reservation_id: Uuid,
    generation: i64,
    issued_at: auction_time::Timestamp,
    expires_at: auction_time::Timestamp,
) {
    let listing = listing_id.to_string();
    let reservation = reservation_id.to_string();
    let mut transaction = pool.begin().await.expect("开启活动预约事务");
    sqlx::query(
        "INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id, canonical_eid,
             season_window_id, generation, service_epoch, session_id, token_digest, issued_at,
             expires_at, state, last_command_id, retired_at, reason)
         VALUES ($1::uuid, $2::uuid, $3::uuid, $4, '12345', 'main', '42', NULL, $5, 1,
                 $6::uuid, decode(repeat('55', 32), 'hex'), $7, $8, 'active',
                 (SELECT command_id FROM public.consignment_commands
                  WHERE listing_id=$2::uuid AND operation='register'
                  ORDER BY created_at DESC LIMIT 1), NULL,
                 NULL)",
    )
    .bind(&reservation)
    .bind(&listing)
    .bind(USER)
    .bind(USER_NAME)
    .bind(generation)
    .bind(MAIN_SESSION)
    .bind(issued_at)
    .bind(expires_at)
    .execute(&mut *transaction)
    .await
    .expect("写入活动预约");
    sqlx::query(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '42', 1, $1::uuid, NULL, $2, $1::uuid)
         ON CONFLICT (world_id, canonical_eid) DO UPDATE
         SET reservation_id=excluded.reservation_id, position_id=NULL,
             revision=public.consignment_equipment_claims.revision + 1,
             changed_at=excluded.changed_at, last_fact_id=excluded.last_fact_id",
    )
    .bind(&reservation)
    .bind(issued_at)
    .execute(&mut *transaction)
    .await
    .expect("写入活动强占用");
    transaction.commit().await.expect("提交活动预约");
}
