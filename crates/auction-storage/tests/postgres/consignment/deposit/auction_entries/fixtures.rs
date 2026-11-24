//! 三入口测试共用的固定身份、真实端口装配与最小事实夹具。

use std::sync::Arc;

use auction_application::admin::{
    AdminIntakeError, AdminIntakePolicy, AdminIntakeSourceKind, AdminIntakeSourceSnapshot,
    EquipmentKeySealerPort,
};
use auction_application::consignment::{
    ReservationSecretAad, ReservationSecretCrypto, ReservationSecretCryptoError,
    SealedReservationSecret,
};
use auction_application::core::{Clock, RequestHash};
use auction_application::mail::{
    MailAuctionIntakeCommand, MailAuctionIntakeService, MailAuctionItem, mail_auction_aggregate_id,
};
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_domain::consignment::deposit::{TokenText, format_token, parse_token};
use auction_domain::consignment::{FeeBps, ListingPrice};
use auction_domain::{CommandId, EquipmentId, EquipmentKey, EquipmentKeyEnvelope, SettlementWorld};
use auction_storage::{DatabaseStore, DepositRepository, DepositSettings, TestDepositLocks};
use auction_time::{Duration, Timestamp};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::support::{
    ACCEPTED, DECIDE, DECISION, ENVELOPE, LISTING, RESERVATION, SESSION, USER_1,
    listing_awaiting_deposit, mail_session, observation_and_asset, reservation_issued, succeeds,
    users_and_key,
};
use crate::recovery_returns::{CodSale, insert_sale};
use crate::support::fixtures;

/// 测试 AEAD：与 4B 夹具同形，只证明端口与真实数据库接线。
pub(super) struct TestCrypto;

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

/// 固定时钟：命令与夹具使用同一时刻，避免测试依赖进程走到下一秒。
#[derive(Clone, Copy)]
pub(super) struct TestClock(pub(super) Timestamp);

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

/// 只返回固定信封的密钥封装替身。
pub(super) struct TestSealer;

impl EquipmentKeySealerPort for TestSealer {
    fn seal(
        &self,
        _equipment_id: EquipmentId,
        _key: &EquipmentKey,
    ) -> Result<EquipmentKeyEnvelope, AdminIntakeError> {
        Ok(EquipmentKeyEnvelope::new(
            vec![0xa1; 28],
            vec![0xb2; 12],
            "fake-data-key".into(),
            "ab".repeat(32),
        )
        .expect("固定信封"))
    }
}

/// 固定外部来源：每次读取返回指定原文 EID 的合法公开快照。
pub(super) struct TestSource {
    pub(super) raw_eid: String,
    pub(super) observed_at: Timestamp,
}

impl auction_application::admin::AdminIntakeSourceReader for TestSource {
    fn read_url(
        &self,
        world_id: String,
        _equipment_url: String,
    ) -> auction_application::core::ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        let raw_eid = self.raw_eid.clone();
        let observed_at = self.observed_at;
        Box::pin(async move { Ok(snapshot(&world_id, &raw_eid, observed_at)) })
    }

    fn read_json(
        &self,
        world_id: String,
        _equipment: auction_application::admin::CanonicalEquipment,
    ) -> auction_application::core::ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        let raw_eid = self.raw_eid.clone();
        let observed_at = self.observed_at;
        Box::pin(async move { Ok(snapshot(&world_id, &raw_eid, observed_at)) })
    }
}

fn snapshot(world_id: &str, raw_eid: &str, observed_at: Timestamp) -> AdminIntakeSourceSnapshot {
    AdminIntakeSourceSnapshot {
        adapter: "test".to_owned(),
        source_fingerprint: [7; 32],
        external_equipment_id: raw_eid.to_owned(),
        name: format!("Fake Gear {world_id}"),
        category: "Weapon".to_owned(),
        level: Some(100),
        tradeable: true,
        attributes: json!({}),
        collected_at: observed_at,
        key: EquipmentKey::new("0123456789").expect("固定 key"),
    }
}

pub(super) fn mail_service(
    store: DatabaseStore,
    now: Timestamp,
) -> MailAuctionIntakeService<DatabaseStore, TestSealer, TestClock> {
    MailAuctionIntakeService::new(Arc::new(store), Arc::new(TestSealer), Arc::new(TestClock(now)))
}

pub(super) fn deposit_repository(store: DatabaseStore) -> DepositRepository {
    DepositRepository::new(
        store,
        Arc::new(TestCrypto),
        DepositSettings::new(
            FeeBps::new(500).expect("费率"),
            ListingPrice::new(ListingPrice::MIN).expect("下限"),
            ListingPrice::new(ListingPrice::MAX).expect("上限"),
            24,
            100,
        ),
    )
}

/// 按寄售会话锁 owner 计算测试用实物锁键。
pub(super) fn equipment_key(identity: &ListingEquipmentIdentity) -> (i32, i32) {
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

/// 由测试连接持有实物会话锁；返回值保持连接存活。
pub(super) async fn hold_equipment_lock(
    pool: &PgPool,
    key: (i32, i32),
) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let mut connection = pool.acquire().await.expect("持有锁连接");
    sqlx::query("SELECT pg_advisory_lock($1, $2)")
        .bind(key.0)
        .bind(key.1)
        .execute(&mut *connection)
        .await
        .expect("取得测试会话锁");
    connection
}

/// 用独立连接尝试取得实物锁；成功时立即释放，返回是否可取得。
pub(super) async fn try_equipment_lock(pool: &PgPool, key: (i32, i32)) -> bool {
    let mut connection = pool.acquire().await.expect("探针连接");
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
        .bind(key.0)
        .bind(key.1)
        .fetch_one(&mut *connection)
        .await
        .expect("尝试测试会话锁");
    if locked {
        let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1, $2)")
            .bind(key.0)
            .bind(key.1)
            .fetch_one(&mut *connection)
            .await
            .expect("释放探针锁");
        assert!(released, "探针必须实际持有并释放锁");
    }
    locked
}

pub(super) type AdminService = auction_application::admin::AdminIntakeService<
    DatabaseStore,
    TestSource,
    TestSealer,
    TestClock,
>;

pub(super) fn admin_service(store: DatabaseStore, raw_eid: &str, now: Timestamp) -> AdminService {
    auction_application::admin::AdminIntakeService::new(
        Arc::new(store),
        Arc::new(TestSource {
            raw_eid: raw_eid.to_owned(),
            observed_at: now - Duration::minutes(1),
        }),
        Arc::new(TestSealer),
        Arc::new(TestClock(now)),
        AdminIntakePolicy::new(false),
    )
}

pub(super) fn mail_command(
    world_id: &str,
    raw_eids: &[&str],
    seed: u8,
    now: Timestamp,
) -> MailAuctionIntakeCommand {
    let received_at = now - Duration::minutes(1);
    MailAuctionIntakeCommand {
        command_id: CommandId::from_uuid(Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("test/mail-auction-intake/{world_id}/{seed}").as_bytes(),
        )),
        aggregate_id: mail_auction_aggregate_id(world_id),
        request_hash: RequestHash::new([seed.max(1); 32]),
        world_id: world_id.to_owned(),
        mailbox_key: format!("mooglemail:{world_id}"),
        external_mail_id: format!("{}", 7_000_000 + u64::from(seed)),
        seller: "FakeSellerOne".to_owned(),
        received_at,
        content_digest: [seed.max(1); 32],
        items: raw_eids
            .iter()
            .enumerate()
            .map(|(index, raw_eid)| MailAuctionItem {
                external_equipment_id: (*raw_eid).to_owned(),
                name: format!("Fake Mail Gear {seed}-{index}"),
                category: "Weapon".to_owned(),
                level: Some(100),
                attributes: json!({}),
                source_fingerprint: [seed.max(1); 32],
                collected_at: received_at,
                start_price: 100,
                key: EquipmentKey::new("0123456789").expect("固定 key"),
            })
            .collect(),
        now,
    }
}

pub(super) fn admin_command(
    world_id: &str,
    raw_eid: &str,
    _now: Timestamp,
) -> auction_application::admin::AdminIntakeCommand {
    auction_application::admin::AdminIntakeCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        world_id: world_id.to_owned(),
        source_kind: Some(AdminIntakeSourceKind::Url),
        equipment_url: Some(format!("https://example.invalid/fake/{raw_eid}")),
        equipment: None,
        start_price: 100,
        player_id: "FakeSellerOne".to_owned(),
        actor_id: "admin".to_owned(),
        request_id: None,
    }
}

/// 插入 seller 用户、未来待寄挂单、邮件会话与仍有效的预约。
pub(super) async fn seed_reservation_fixture(pool: &PgPool) {
    let listing =
        listing_awaiting_deposit().replace("2026-01-02T00:00:00Z", "2099-12-31T00:00:00Z");
    let reservation = reservation_issued().replace("2026-01-01T12:00:00Z", "2099-01-01T12:00:00Z");
    succeeds(pool, &(users_and_key() + &listing + &mail_session() + &mail_ready() + &reservation))
        .await;
}

/// 插入 main 收件就绪与解析闸门；预约签发要求真实 readiness 与 allowlist。
fn mail_ready() -> String {
    format!(
        "INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('main', '{SESSION}', 1, decode(repeat('35', 32), 'hex'), 'enforced',
                 ARRAY['FakeSellerOne'], ARRAY[]::text[], true, true, true,
                 clock_timestamp(), clock_timestamp() + interval '1 day', 1);
         UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1;"
    )
}

/// 仅插入未来待寄挂单与收件就绪，不签发预约；用于预约与拍卖入口的真实竞争。
pub(super) async fn seed_awaiting_without_reservation(pool: &PgPool) {
    let listing =
        listing_awaiting_deposit().replace("2026-01-02T00:00:00Z", "2099-12-31T00:00:00Z");
    succeeds(pool, &(users_and_key() + &listing + &mail_session() + &mail_ready())).await;
}

/// 插入活动预约强占用；返回预约 UUID。
pub(super) async fn seed_reservation_claim(pool: &PgPool) -> Uuid {
    let reservation = Uuid::parse_str(RESERVATION).expect("固定预约");
    let listing =
        listing_awaiting_deposit().replace("2026-01-02T00:00:00Z", "2099-12-31T00:00:00Z");
    let issued = reservation_issued().replace("2026-01-01T12:00:00Z", "2099-01-01T12:00:00Z");
    succeeds(
        pool,
        &format!(
            "{}{}{}{}{}
             INSERT INTO public.consignment_equipment_claims
                (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
                 last_fact_id)
             VALUES ('main', '42', 1, '{RESERVATION}', NULL, '2026-01-01T02:00:00Z',
                     '{RESERVATION}');",
            users_and_key(),
            listing,
            mail_session(),
            mail_ready(),
            issued,
        ),
    )
    .await;
    reservation
}

/// 通过真实 4B 接受原语把预约无空档转为 position 持有；返回 position UUID。
pub(super) async fn seed_position_claim(database: &crate::support::TestDatabase) -> Uuid {
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    seed_reservation_claim(pool).await;
    succeeds(pool, &observation_and_asset()).await;
    let repository = deposit_repository(store);
    let item = auction_domain::consignment::deposit::MatchedDepositItem {
        ordinal: 1,
        listing_id: Uuid::parse_str(LISTING).expect("固定挂单"),
        listing_version: auction_domain::consignment::ListingVersion::new(3).expect("版本"),
        seller_user_id: auction_domain::UserId::from_uuid(
            Uuid::parse_str(USER_1).expect("固定卖家"),
        ),
        reservation_id: Uuid::parse_str(RESERVATION).expect("固定预约"),
        reservation_generation: 1,
        equipment: auction_domain::consignment::ListingEquipmentIdentity::new("main", "42")
            .expect("固定装备"),
        credential_ref: Some(Uuid::parse_str(ENVELOPE).expect("固定凭证")),
    };
    let accepted = repository
        .test_accept_items_at(
            Uuid::parse_str(DECIDE).expect("决定命令"),
            Uuid::parse_str(DECISION).expect("决定"),
            auction_domain::consignment::OriginalMailKey::new(
                "12345",
                SettlementWorld::Main,
                "555",
            )
            .expect("原信键"),
            vec![item],
            Timestamp::parse_from_rfc3339(ACCEPTED).expect("接受时刻"),
        )
        .await
        .expect("整封接受");
    accepted.first().expect("一个 position").position_id
}

/// 为指定世界插入一个 draft 收件目标；返回 auction UUID。
pub(super) async fn seed_draft_target(pool: &PgPool, world_id: &str, sequence: i64) -> Uuid {
    let auction_id = Uuid::new_v4();
    let at = Timestamp::now() - Duration::hours(1);
    let mut transaction = pool.begin().await.expect("开启 draft 目标事务");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1, $2, $3, 'Fake entry target', 'draft', $4, $4, $4, $4, $5)",
    )
    .bind(auction_id)
    .bind(world_id)
    .bind(sequence)
    .bind(at)
    .bind(fixtures::policy_snapshot(at + Duration::days(1), 50_000, 30_000))
    .execute(&mut *transaction)
    .await
    .expect("写入 draft 场次");
    sqlx::query(
        "INSERT INTO public.auction_intake_targets (world_id, auction_id, target_status)
         VALUES ($1, $2, 'draft')",
    )
    .bind(world_id)
    .bind(auction_id)
    .execute(&mut *transaction)
    .await
    .expect("写入 draft 收件目标");
    transaction.commit().await.expect("提交 draft 目标");
    auction_id
}

pub(super) async fn count_lots(pool: &PgPool, auction_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.lots WHERE auction_id = $1")
        .bind(auction_id)
        .fetch_one(pool)
        .await
        .expect("读取 lot 数")
}

pub(super) async fn count_equipment_lots(pool: &PgPool, equipment_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.lots WHERE equipment_id = $1")
        .bind(equipment_id)
        .fetch_one(pool)
        .await
        .expect("读取装备 lot 数")
}

/// 可确认的重列恢复记录身份。
pub(super) struct ConfirmableSale {
    pub(super) sale: CodSale,
    pub(super) recovery_id: Uuid,
}

/// 通过既有结算夹具构造一条 `reclaiming` 恢复记录，补齐确认所需的送件、退件、结算与动作事实。
pub(super) async fn seed_confirmable_recovery(
    pool: &PgPool,
    sequence: i64,
    external_eid: &str,
) -> ConfirmableSale {
    let at = Timestamp::now() - Duration::hours(2);
    let sale = insert_sale(pool, sequence, at, 1).await;
    sqlx::query("UPDATE public.equipment SET external_equipment_id=$2 WHERE id=$1")
        .bind(sale.equipment_id)
        .bind(external_eid)
        .execute(pool)
        .await
        .expect("改写测试装备编号");
    sqlx::query("UPDATE public.settlement_obligations SET business_state='blocked' WHERE id=$1")
        .bind(sale.seller_obligation)
        .execute(pool)
        .await
        .expect("卖方货款待撤销");

    let submission_id = Uuid::new_v4();
    let source_command = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'relist-fixture', $2, $3, 'accepted')",
    )
    .bind(source_command)
    .bind(sale.auction_id)
    .bind([3_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入投稿命令");
    sqlx::query(
        "INSERT INTO public.auction_submissions
            (id, source_command_id, auction_id, seller, received_at, source_kind,
             source_reference, source_fingerprint)
         VALUES ($1, $2, $3, 'FakeSellerOne', $4, 'admin', 'relist-fixture', $5)",
    )
    .bind(submission_id)
    .bind(source_command)
    .bind(sale.auction_id)
    .bind(at - Duration::hours(1))
    .bind([4_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入投稿");
    sqlx::query("UPDATE public.lots SET submission_id=$2 WHERE id=$1")
        .bind(sale.lot_id)
        .bind(submission_id)
        .execute(pool)
        .await
        .expect("绑定投稿");
    sqlx::query(
        "INSERT INTO public.auction_settlements
            (auction_id, command_id, policy_version, policy_mode, pool_enabled,
             opening_pool_balance, closing_pool_balance, settled_at)
         VALUES ($1, $2, 0, 'legacy_zero_v0', false, 0, 0, $3)",
    )
    .bind(sale.auction_id)
    .bind(source_command)
    .bind(at)
    .execute(pool)
    .await
    .expect("写入场次结算");
    sqlx::query(
        "INSERT INTO public.lot_settlements
            (lot_id, end_price, listed_price, buyer_amount, seller_amount, tax_amount,
             compensation_amount, hold_ppm, adjustment_ppm)
         VALUES ($1, 100, 100, 100, 100, 0, 0, 0, 0)",
    )
    .bind(sale.lot_id)
    .execute(pool)
    .await
    .expect("写入 lot 结算");

    let payment_delivery = Uuid::new_v4();
    let prepare_command = Uuid::new_v4();
    let activate_command = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at, result_metadata,
             activation_command_id, activation_request_digest, activated_at)
         VALUES ($1, $2, $3, 'settlement', $4, 'Recovery buyer',
                 jsonb_build_object('schema_version', 1, 'operation', 'send', 'world', 'main',
                     'template', 'settlement_winner_equipment_cod', 'credits', 0, 'cod', 100,
                     'variables', jsonb_build_object('reference', $4),
                     'equipment_secret_refs', '[]'::jsonb),
                 'succeeded', 2, 1, $5, $5, $5, $6, $5, $5,
                 '{\"operation\":\"send\"}'::jsonb, $7, $8, $5)",
    )
    .bind(payment_delivery)
    .bind(prepare_command)
    .bind([5_u8; 32].as_slice())
    .bind(sale.payment_obligation.to_string())
    .bind(at + Duration::minutes(5))
    .bind(format!("mooglemail:main:send:{}", 770_000_000 + sequence))
    .bind(activate_command)
    .bind([6_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入原付款投递");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, state_version, dispatch_world, delivery_operation,
             delivery_id, prepare_command_id, prepare_request_digest, activate_command_id,
             activate_request_digest, created_at, updated_at)
         VALUES ($1, 'enqueued', 3, 'main', 'send', $2, $3, $4, $5, $6, $7, $7)",
    )
    .bind(sale.payment_obligation)
    .bind(payment_delivery)
    .bind(prepare_command)
    .bind([5_u8; 32].as_slice())
    .bind(activate_command)
    .bind([6_u8; 32].as_slice())
    .bind(at + Duration::minutes(5))
    .execute(pool)
    .await
    .expect("写入原付款履约");
    for (obligation, dormant) in [(sale.seller_obligation, false), (sale.refund_obligation, true)] {
        sqlx::query(
            "INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id, execution_state, dispatch_world, delivery_operation, dormant,
                 created_at, updated_at)
             VALUES ($1, 'pending', 'main', 'send', $2, $3, $3)",
        )
        .bind(obligation)
        .bind(dormant)
        .bind(at)
        .execute(pool)
        .await
        .expect("写入依赖履约");
    }

    let return_mail = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at, sender, subject)
         VALUES ($1, 'relist-fixture', $2, 'main', 'delivery', $3, $4, 'accepted',
                 'settlement_recovery_returned', $5, $4, 'MoogleMail', $6)",
    )
    .bind(return_mail)
    .bind(format!("{}", 650_000 + sequence))
    .bind([7_u8; 32].as_slice())
    .bind(at + Duration::minutes(30))
    .bind(json!({
        "obligation_id": sale.payment_obligation.to_string(),
        "credits": 0,
        "cod": 0,
        "attachment_ids": [external_eid],
    }))
    .bind(format!("[RTS] Auction equipment delivery [{}]", sale.payment_obligation))
    .execute(pool)
    .await
    .expect("写入系统退件");

    let recovery_id = Uuid::new_v4();
    let custody_observed_at = at + Duration::minutes(45);
    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, seller_payout_obligation_id, buyer_refund_obligation_id,
             policy_version, payment_deadline_at, stage, state_version, return_mail_id,
             return_external_mail_id, custody_proof_kind, custody_proof_reference,
             custody_proof_digest, custody_proof_observed_at, cod_return_count, cod_return_limit,
             created_at, updated_at)
         VALUES ($1, $2, $3, 'main', $4, $5, $6, $7, $8, 1, $9 + interval '7 days',
                 'reclaiming', 5, $10, $11, 'delivery_recovery_take_succeeded',
                 'fake:take', $12, $13, 3, 3, $9, $13)",
    )
    .bind(recovery_id)
    .bind(sale.auction_id)
    .bind(sale.lot_id)
    .bind(sale.equipment_id)
    .bind(sale.winner_user_id)
    .bind(sale.payment_obligation)
    .bind(sale.seller_obligation)
    .bind(sale.refund_obligation)
    .bind(at)
    .bind(return_mail)
    .bind(format!("{}", 650_000 + sequence))
    .bind([8_u8; 32].as_slice())
    .bind(custody_observed_at)
    .execute(pool)
    .await
    .expect("写入 reclaiming 恢复记录");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_consumptions
            (return_mail_id, recovery_id, recorded_at) VALUES ($1, $2, $3)",
    )
    .bind(return_mail)
    .bind(recovery_id)
    .bind(at + Duration::minutes(31))
    .execute(pool)
    .await
    .expect("写入退件消费");

    let take_delivery = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at, result_metadata)
         VALUES ($1, $2, $3, 'settlement_recovery_action', $4, 'MoogleMail',
                 jsonb_build_object('schema_version', 1, 'operation', 'take', 'world', 'main',
                     'external_mail_id', $5::text),
                 'succeeded', 1, 1, $6, $6, $6, $7, $6, $6, '{\"operation\":\"take\"}'::jsonb)",
    )
    .bind(take_delivery)
    .bind(Uuid::new_v4())
    .bind([9_u8; 32].as_slice())
    .bind(format!("recovery:{recovery_id}"))
    .bind(format!("{}", 650_000 + sequence))
    .bind(custody_observed_at)
    .bind(format!("mooglemail:main:take:{}", 650_000 + sequence))
    .execute(pool)
    .await
    .expect("写入收回投递");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_actions
            (id, recovery_id, kind, ordinal, execution_state, state_version, source_mail_id,
             delivery_id, proof_kind, proof_reference, proof_digest, proof_observed_at,
             created_at, updated_at, completed_at)
         VALUES ($1, $2, 'reclaim_equipment_take', 1, 'satisfied', 3, $3, $4,
                 'delivery_recovery_take_succeeded', 'fake:take', $5, $6, $7, $6, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(recovery_id)
    .bind(return_mail)
    .bind(take_delivery)
    .bind([8_u8; 32].as_slice())
    .bind(custody_observed_at)
    .bind(at + Duration::minutes(31))
    .execute(pool)
    .await
    .expect("写入收回动作");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_actions
            (id, recovery_id, kind, ordinal, execution_state, state_version, proof_kind,
             proof_reference, proof_digest, proof_observed_at, reason, created_at, updated_at,
             completed_at)
         VALUES ($1, $2, 'buyer_blacklist', 2, 'satisfied', 2, 'users_blacklist_applied',
                 $3, $4, $5, 'COD returned repeatedly', $6, $5, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(recovery_id)
    .bind(sale.winner_user_id.to_string())
    .bind([10_u8; 32].as_slice())
    .bind(custody_observed_at)
    .bind(at + Duration::minutes(31))
    .execute(pool)
    .await
    .expect("写入拉黑动作");
    ConfirmableSale { sale, recovery_id }
}

/// 构造一条已满足 custody 证明、等待自动重列的恢复记录。
pub(super) async fn seed_scheduled_relist(
    pool: &PgPool,
    sequence: i64,
    external_eid: &str,
) -> (CodSale, Uuid) {
    let at = Timestamp::now() - Duration::hours(2);
    let sale = insert_sale(pool, sequence, at, 1).await;
    sqlx::query("UPDATE public.equipment SET external_equipment_id=$2 WHERE id=$1")
        .bind(sale.equipment_id)
        .bind(external_eid)
        .execute(pool)
        .await
        .expect("改写测试装备编号");
    let submission_id = Uuid::new_v4();
    let source_command = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'relist-fixture', $2, $3, 'accepted')",
    )
    .bind(source_command)
    .bind(sale.auction_id)
    .bind([11_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入投稿命令");
    sqlx::query(
        "INSERT INTO public.auction_submissions
            (id, source_command_id, auction_id, seller, received_at, source_kind,
             source_reference, source_fingerprint)
         VALUES ($1, $2, $3, 'FakeSellerOne', $4, 'admin', 'relist-fixture', $5)",
    )
    .bind(submission_id)
    .bind(source_command)
    .bind(sale.auction_id)
    .bind(at - Duration::hours(1))
    .bind([12_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入投稿");
    sqlx::query("UPDATE public.lots SET submission_id=$2 WHERE id=$1")
        .bind(sale.lot_id)
        .bind(submission_id)
        .execute(pool)
        .await
        .expect("绑定投稿");

    let recovery_id = Uuid::new_v4();
    let return_mail = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, processed_at, sender, subject)
         VALUES ($1, 'relist-fixture', $2, 'main', 'delivery', $3, $4, 'accepted',
                 'settlement_recovery_returned', $4, 'MoogleMail', $5)",
    )
    .bind(return_mail)
    .bind(format!("{}", 660_000 + sequence))
    .bind([13_u8; 32].as_slice())
    .bind(at + Duration::minutes(30))
    .bind(format!("[RTS] Auction equipment delivery [{}]", sale.payment_obligation))
    .execute(pool)
    .await
    .expect("写入系统退件");
    let delivery_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at, result_metadata)
         VALUES ($1, $2, $3, 'settlement_recovery_action', $4, 'MoogleMail',
                 jsonb_build_object('schema_version', 1, 'operation', 'take', 'world', 'main',
                     'external_mail_id', $5::text),
                 'succeeded', 1, 1, $6, $6, $6, $7, $6, $6, '{\"operation\":\"take\"}'::jsonb)",
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind([14_u8; 32].as_slice())
    .bind(action_id.to_string())
    .bind(format!("{}", 660_000 + sequence))
    .bind(at + Duration::minutes(45))
    .bind(format!("mooglemail:main:take:{}", 660_000 + sequence))
    .execute(pool)
    .await
    .expect("写入收回投递");
    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, seller_payout_obligation_id, buyer_refund_obligation_id,
             policy_version, payment_deadline_at, stage, resolution, state_version,
             return_mail_id, return_external_mail_id, custody_proof_kind,
             custody_proof_reference, custody_proof_digest, custody_proof_observed_at,
             cod_return_count, cod_return_limit, next_attempt_at, created_at, updated_at)
         VALUES ($1, $2, $3, 'main', $4, $5, $6, $7, $8, 1, $9 + interval '7 days',
                 'relist_pending', 'reclaimed', 5, $10, $11,
                 'delivery_recovery_take_succeeded', 'fake:take', $12, $13, 3, 3, $9, $9, $13)",
    )
    .bind(recovery_id)
    .bind(sale.auction_id)
    .bind(sale.lot_id)
    .bind(sale.equipment_id)
    .bind(sale.winner_user_id)
    .bind(sale.payment_obligation)
    .bind(sale.seller_obligation)
    .bind(sale.refund_obligation)
    .bind(at)
    .bind(return_mail)
    .bind(format!("{}", 660_000 + sequence))
    .bind([15_u8; 32].as_slice())
    .bind(at + Duration::minutes(45))
    .execute(pool)
    .await
    .expect("写入待重列恢复记录");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_actions
            (id, recovery_id, kind, ordinal, execution_state, state_version, source_mail_id,
             delivery_id, proof_kind, proof_reference, proof_digest, proof_observed_at,
             created_at, updated_at, completed_at)
         VALUES ($1, $2, 'reclaim_equipment_take', 1, 'satisfied', 3, $3, $4,
                 'delivery_recovery_take_succeeded', 'fake:take', $5, $6, $7, $6, $6)",
    )
    .bind(action_id)
    .bind(recovery_id)
    .bind(return_mail)
    .bind(delivery_id)
    .bind([15_u8; 32].as_slice())
    .bind(at + Duration::minutes(45))
    .bind(at + Duration::minutes(31))
    .execute(pool)
    .await
    .expect("写入收回动作");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_actions
            (id, recovery_id, kind, ordinal, execution_state, state_version, proof_kind,
             proof_reference, proof_digest, proof_observed_at, reason, created_at, updated_at,
             completed_at)
         VALUES ($1, $2, 'buyer_blacklist', 2, 'satisfied', 2, 'users_blacklist_applied',
                 $3, $4, $5, 'COD returned repeatedly', $6, $5, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(recovery_id)
    .bind(sale.winner_user_id.to_string())
    .bind([16_u8; 32].as_slice())
    .bind(at + Duration::minutes(45))
    .bind(at + Duration::minutes(31))
    .execute(pool)
    .await
    .expect("写入拉黑动作");
    (sale, recovery_id)
}
