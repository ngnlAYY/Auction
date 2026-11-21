//! 真 PG18 验证装备域跨表轮换、并发围栏与已受理登记的重放。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ListingEquipmentKeyOpener, ListingRegistrationSecret, RegistrationInput,
    RegistrationReplayCrypto, RegistrationRequestMac, WrappedRegistrationReplayKey,
};
use auction_domain::consignment::{FeeBps, ListingEquipmentIdentity, ListingMode, ListingPrice};
use auction_domain::{DataKeyDomain, EquipmentId, EquipmentKey, EquipmentKeyEnvelope};
use auction_integrations::{DataKey, EquipmentKeyOpener, EquipmentKeySealer, Keyring};
use auction_storage::{
    RotatedKeyEnvelope, StoredKeyEnvelope, replace_key_envelope, scan_key_envelopes, stored_key_ids,
};
use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::support::TestDatabase;
use crate::support::fixtures;

const OLD: &str = "fake-rotation-old";
const CURRENT: &str = "fake-rotation-current";
const SHARED: Uuid = Uuid::from_u128(41);
const SECOND: Uuid = Uuid::from_u128(42);
const REGISTRATION_AAD: Uuid = Uuid::from_u128(81);
const SELLER: Uuid = Uuid::from_u128(91);
const COMMAND: Uuid = Uuid::from_u128(92);

fn ring(current: &str, include_old: bool) -> Arc<Keyring> {
    let old = || {
        Arc::new(
            DataKey::from_base64(OLD, "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=")
                .expect("假钥合法"),
        )
    };
    if current == OLD {
        return Arc::new(Keyring::single(DataKeyDomain::Equipment, old()));
    }
    let current = Arc::new(
        DataKey::from_base64(CURRENT, "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=")
            .expect("假钥合法"),
    );
    let previous = include_old.then(old).into_iter().collect();
    Arc::new(Keyring::new(DataKeyDomain::Equipment, current, previous).expect("装备密钥环"))
}

async fn seed_replay(
    pool: &PgPool,
    replay_key_id: Uuid,
    wrapped: &WrappedRegistrationReplayKey,
    write_current: bool,
) {
    sqlx::query(
        "INSERT INTO public.consignment_replay_keys
         (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, transaction_timestamp(), transaction_timestamp())",
    )
    .bind(replay_key_id)
    .bind(write_current)
    .bind(wrapped.wrapped_key())
    .bind(wrapped.nonce().as_slice())
    .bind(wrapped.encryption_key_id())
    .execute(pool)
    .await
    .expect("写入比较钥包装");
}

async fn seed_auction(
    pool: &PgPool,
    equipment_pk: Uuid,
    eid: &str,
    envelope: &EquipmentKeyEnvelope,
) {
    sqlx::query(
        "INSERT INTO public.equipment (id, world_id, external_equipment_id, name, category, collected_at)
         VALUES ($1, 'main', $2, 'Fake equipment', 'Fake category', transaction_timestamp())",
    )
    .bind(equipment_pk)
    .bind(eid)
    .execute(pool)
    .await
    .expect("写入拍卖装备");
    sqlx::query(
        "INSERT INTO public.equipment_secrets
         (equipment_id, ciphertext, nonce, encryption_key_id, fingerprint)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(equipment_pk)
    .bind(envelope.ciphertext())
    .bind(envelope.nonce())
    .bind(envelope.encryption_key_id())
    .bind(envelope.fingerprint())
    .execute(pool)
    .await
    .expect("写入拍卖装备信封");
}

async fn seed_registration(
    pool: &PgPool,
    envelope: &EquipmentKeyEnvelope,
    mac: RegistrationRequestMac,
) {
    fixtures::seed_user(pool, SELLER, "FakeRotationSeller").await;
    let mut tx = pool.begin().await.expect("登记事务");
    sqlx::query(
        "INSERT INTO public.consignment_listings
         (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
          pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
          created_at, updated_at, expires_at)
         VALUES ($1, $2, 'main', '73', 'direct', 'resolving', 101, 1, 0, 1, 1, 0,
                 transaction_timestamp(), transaction_timestamp(), transaction_timestamp() + interval '24 hours')",
    )
    .bind(SHARED)
    .bind(SELLER)
    .execute(&mut *tx)
    .await
    .expect("写入挂单");
    sqlx::query(
        "INSERT INTO public.consignment_commands
         (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
          replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ($1, 'public_user', $2, 'register', 'new', $3, $4, $4, 202, 'accepted',
                 $5, transaction_timestamp())",
    )
    .bind(COMMAND)
    .bind(SELLER.to_string())
    .bind(mac.as_bytes().as_slice())
    .bind(SHARED)
    .bind(json!({
        "command_id": COMMAND, "operation": "register", "listing_id": SHARED,
        "listing_version": 1, "state": "resolving", "disposition": "registered", "price": 101,
    }))
    .execute(&mut *tx)
    .await
    .expect("写入登记账本");
    sqlx::query(
        "INSERT INTO public.consignment_events
         (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
          to_state, reason_code, price, occurred_at)
         VALUES ($1, 1, $2, 'public_user', 'registered', NULL, 'resolving', NULL, 101, transaction_timestamp())",
    )
    .bind(SHARED)
    .bind(COMMAND)
    .execute(&mut *tx)
    .await
    .expect("写入登记事实");
    sqlx::query(
        "INSERT INTO public.consignment_registration_secrets
         (listing_id, aad_equipment_id, ciphertext, nonce, encryption_key_id, fingerprint,
          created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, transaction_timestamp(), transaction_timestamp())",
    )
    .bind(SHARED)
    .bind(REGISTRATION_AAD)
    .bind(envelope.ciphertext())
    .bind(envelope.nonce())
    .bind(envelope.encryption_key_id())
    .bind(envelope.fingerprint())
    .execute(&mut *tx)
    .await
    .expect("写入登记信封");
    tx.commit().await.expect("登记事实同事务提交");
}

async fn retire_registration(pool: &PgPool) {
    sqlx::query(
        "UPDATE public.consignment_registration_secrets SET retired_at = transaction_timestamp(),
                updated_at = transaction_timestamp(), ciphertext = NULL, nonce = NULL,
                encryption_key_id = NULL, fingerprint = NULL WHERE listing_id = $1",
    )
    .bind(SHARED)
    .execute(pool)
    .await
    .expect("退休登记凭证");
}

fn fake_key() -> EquipmentKey {
    EquipmentKey::new("aabbccddee").expect("假凭证合法")
}

fn input<'a>(
    identity: &'a ListingEquipmentIdentity,
    key: &'a EquipmentKey,
) -> RegistrationInput<'a> {
    RegistrationInput {
        identity,
        key,
        mode: ListingMode::Direct,
        price: ListingPrice::new(101).expect("标价合法"),
        expected_fee_bps: FeeBps::new(0).expect("费率合法"),
    }
}

async fn seed_three_sources(pool: &PgPool) -> WrappedRegistrationReplayKey {
    let old = ring(OLD, false);
    let opener = EquipmentKeyOpener::new(Arc::clone(&old));
    let sealer = EquipmentKeySealer::new(Arc::clone(old.current()));
    let key = fake_key();
    let identity = ListingEquipmentIdentity::new("main", "73").expect("规范装备身份");
    let wrapped = opener.generate_wrapped(SHARED).expect("生成逻辑钥");
    let mac = opener.registration_mac(SHARED, &wrapped, &input(&identity, &key)).expect("登记 MAC");
    seed_replay(pool, SHARED, &wrapped, true).await;
    let registration =
        sealer.seal(EquipmentId::from_uuid(REGISTRATION_AAD), &key).expect("封装登记凭证");
    seed_registration(pool, &registration, mac).await;
    let equipment = sealer.seal(EquipmentId::from_uuid(SHARED), &key).expect("封装装备凭证");
    seed_auction(pool, SHARED, "74", &equipment).await;
    wrapped
}

fn wrapped_row(row: &StoredKeyEnvelope) -> WrappedRegistrationReplayKey {
    WrappedRegistrationReplayKey::new(
        row.ciphertext.clone(),
        row.nonce.as_slice().try_into().expect("合法随机数长度"),
        row.encryption_key_id.clone(),
    )
    .expect("合法包装")
}

fn rotated_replay(wrapped: &WrappedRegistrationReplayKey) -> RotatedKeyEnvelope {
    RotatedKeyEnvelope {
        ciphertext: wrapped.wrapped_key().to_vec(),
        nonce: wrapped.nonce().to_vec(),
        encryption_key_id: wrapped.encryption_key_id().to_owned(),
        fingerprint: String::new(),
    }
}

fn rotated_equipment(envelope: &EquipmentKeyEnvelope) -> RotatedKeyEnvelope {
    RotatedKeyEnvelope {
        ciphertext: envelope.ciphertext().to_vec(),
        nonce: envelope.nonce().to_vec(),
        encryption_key_id: envelope.encryption_key_id().to_owned(),
        fingerprint: envelope.fingerprint().to_owned(),
    }
}

async fn scan(pool: &PgPool) -> Vec<StoredKeyEnvelope> {
    scan_key_envelopes(pool, DataKeyDomain::Equipment, CURRENT, None, 200)
        .await
        .expect("扫描旧信封")
}

#[tokio::test]
async fn equipment_rotation_pages_by_source_and_primary_key() {
    let database = TestDatabase::create().await.expect("克隆库");
    let store = database.store().await.expect("连接临时库");
    let pool = store.postgres_pool();
    assert!(scan(pool).await.is_empty());
    assert!(stored_key_ids(pool, DataKeyDomain::Equipment).await.expect("空库引用").is_empty());
    seed_three_sources(pool).await;
    let old = ring(OLD, false);
    let sealer = EquipmentKeySealer::new(Arc::clone(old.current()));
    let extra = sealer.seal(EquipmentId::from_uuid(SECOND), &fake_key()).expect("封装同表第二条");
    seed_auction(pool, SECOND, "75", &extra).await;

    for page_size in [1, 2, 3] {
        let mut cursor = None;
        let mut seen = Vec::new();
        loop {
            let page =
                scan_key_envelopes(pool, DataKeyDomain::Equipment, CURRENT, cursor, page_size)
                    .await
                    .expect("扫描一页");
            assert!(page.len() <= usize::try_from(page_size).expect("页长"));
            let Some(last) = page.last() else { break };
            cursor = Some(last.cursor());
            seen.extend(page.iter().map(|row| (row.source().as_str(), row.entity_id)));
            assert!(seen.len() <= 4, "分页必须推进且不重复");
        }
        assert_eq!(
            seen,
            [
                ("equipment_secrets", SHARED),
                ("equipment_secrets", SECOND),
                ("consignment_registration_secrets", SHARED),
                ("consignment_replay_keys", SHARED),
            ]
        );
    }
    let page = scan(pool).await;
    assert_eq!(page[2].equipment_aad(), Some(REGISTRATION_AAD));
    assert_eq!(page[3].replay_key_id(), Some(SHARED));
    assert!(
        scan_key_envelopes(pool, DataKeyDomain::Equipment, OLD, None, 2)
            .await
            .expect("过滤当前钥")
            .is_empty()
    );
    retire_registration(pool).await;
    assert_eq!(scan(pool).await.len(), 3);
    pool.close().await;
}

#[tokio::test]
async fn equipment_rotation_cas_binds_source_and_primary_key() {
    let database = TestDatabase::create().await.expect("克隆库");
    let store = database.store().await.expect("连接临时库");
    let pool = store.postgres_pool();
    seed_three_sources(pool).await;
    let old = ring(OLD, false);
    let sealer = EquipmentKeySealer::new(Arc::clone(old.current()));
    let copied_fields =
        sealer.seal(EquipmentId::from_uuid(SECOND), &fake_key()).expect("封装假凭证");
    seed_auction(pool, SECOND, "75", &copied_fields).await;
    sqlx::query("UPDATE public.equipment_secrets SET nonce = (SELECT nonce FROM public.equipment_secrets WHERE equipment_id = $1) WHERE equipment_id = $2")
        .bind(SHARED).bind(SECOND).execute(pool).await.expect("两个测试主键使用相同 CAS 字段");

    let page = scan(pool).await;
    let current = ring(CURRENT, false);
    let sealer = EquipmentKeySealer::new(Arc::clone(current.current()));
    let replacement = sealer.seal(EquipmentId::from_uuid(SHARED), &fake_key()).expect("重封");
    let replacement = rotated_equipment(&replacement);
    assert!(
        replace_key_envelope(pool, &page[0], &replacement).await.expect("正确 CAS").was_replaced()
    );
    let remaining = scan(pool).await;
    assert_eq!(
        remaining.iter().map(|row| (row.source().as_str(), row.entity_id)).collect::<Vec<_>>(),
        [
            ("equipment_secrets", SECOND),
            ("consignment_registration_secrets", SHARED),
            ("consignment_replay_keys", SHARED),
        ]
    );
    pool.close().await;
}

#[tokio::test]
async fn equipment_rotation_cas_updates_only_the_selected_table_for_shared_uuid() {
    let database = TestDatabase::create().await.expect("克隆库");
    let store = database.store().await.expect("连接临时库");
    let pool = store.postgres_pool();
    seed_three_sources(pool).await;
    let page = scan(pool).await;
    let current = ring(CURRENT, false);
    let replacement = EquipmentKeySealer::new(Arc::clone(current.current()))
        .seal(EquipmentId::from_uuid(REGISTRATION_AAD), &fake_key())
        .expect("重封登记凭证");
    assert!(
        replace_key_envelope(pool, &page[1], &rotated_equipment(&replacement))
            .await
            .expect("从登记表写回")
            .was_replaced()
    );
    let remaining = scan(pool).await;
    assert_eq!(
        remaining.iter().map(|row| (row.source().as_str(), row.entity_id)).collect::<Vec<_>>(),
        [("equipment_secrets", SHARED), ("consignment_replay_keys", SHARED)]
    );
    pool.close().await;
}

#[tokio::test]
async fn equipment_rotation_references_count_all_three_sources_but_not_retired_secrets() {
    let database = TestDatabase::create().await.expect("克隆库");
    let store = database.store().await.expect("连接临时库");
    let pool = store.postgres_pool();
    seed_three_sources(pool).await;
    assert!(
        stored_key_ids(pool, DataKeyDomain::Equipment).await.expect("三处来源合计")
            == [(OLD.to_owned(), 3)]
    );
    retire_registration(pool).await;
    assert!(
        stored_key_ids(pool, DataKeyDomain::Equipment).await.expect("退休后引用")
            == [(OLD.to_owned(), 2)]
    );
    pool.close().await;
}

async fn assert_stale_cas_is_rejected(change_nonce: bool) {
    let database = TestDatabase::create().await.expect("克隆库");
    let store = database.store().await.expect("连接临时库");
    let pool = store.postgres_pool();
    let old = ring(OLD, false);
    let current = ring(CURRENT, false);
    let aad = EquipmentId::from_uuid(SHARED);
    let secret = fake_key();
    let envelope =
        EquipmentKeySealer::new(Arc::clone(old.current())).seal(aad, &secret).expect("原信封");
    seed_auction(pool, SHARED, "74", &envelope).await;
    let scanned = scan(pool).await;
    let envelope =
        EquipmentKeySealer::new(Arc::clone(current.current())).seal(aad, &secret).expect("新信封");
    let changed_key = if change_nonce { OLD } else { "fake-rotation-other-key" };
    let mut changed_nonce = scanned[0].nonce.clone();
    if change_nonce {
        changed_nonce[0] ^= 1;
    }
    sqlx::query("UPDATE public.equipment_secrets SET encryption_key_id = $1, nonce = $2 WHERE equipment_id = $3")
        .bind(changed_key)
        .bind(&changed_nonce)
        .bind(SHARED)
        .execute(pool)
        .await
        .expect("扫描后并发改动原信封");
    assert!(matches!(
        replace_key_envelope(pool, &scanned[0], &rotated_equipment(&envelope)).await,
        Err(auction_storage::KeyRotationError::Conflict)
    ));
    let now = scan(pool).await;
    assert_eq!(now.len(), 1);
    assert!(now[0].encryption_key_id == changed_key && now[0].nonce == changed_nonce);
    pool.close().await;
}

#[tokio::test]
async fn equipment_rotation_cas_rejects_changed_old_key_id() {
    assert_stale_cas_is_rejected(false).await;
}

#[tokio::test]
async fn equipment_rotation_cas_rejects_changed_nonce() {
    assert_stale_cas_is_rejected(true).await;
}

#[tokio::test]
async fn equipment_rotation_rechecks_retirement_without_reviving_envelope() {
    let database = TestDatabase::create().await.expect("克隆库");
    let store = database.store().await.expect("连接临时库");
    let pool = store.postgres_pool();
    seed_three_sources(pool).await;
    let page = scan(pool).await;
    let current = ring(CURRENT, false);
    let replacement = EquipmentKeySealer::new(Arc::clone(current.current()))
        .seal(EquipmentId::from_uuid(REGISTRATION_AAD), &fake_key())
        .expect("预先重封活动信封");
    retire_registration(pool).await;
    let skipped = replace_key_envelope(pool, &page[1], &rotated_equipment(&replacement))
        .await
        .expect("零行后复核已经退休");
    assert!(!skipped.was_replaced());
    assert!(skipped.current_envelope().is_none());
    let erased: bool = sqlx::query_scalar(
        "SELECT aad_equipment_id = $1 AND retired_at = updated_at AND ciphertext IS NULL
                AND nonce IS NULL AND encryption_key_id IS NULL AND fingerprint IS NULL
         FROM public.consignment_registration_secrets WHERE listing_id = $2",
    )
    .bind(REGISTRATION_AAD)
    .bind(SHARED)
    .fetch_one(pool)
    .await
    .expect("读取退休事实");
    assert!(erased);
    pool.close().await;
}

#[tokio::test]
async fn equipment_rotation_two_processes_only_one_cas_wins() {
    let database = TestDatabase::create().await.expect("克隆库");
    let first_store = database.store().await.expect("连接临时库");
    let second_store = database.store().await.expect("独立连接池");
    let pool = first_store.postgres_pool();
    let second_pool = second_store.postgres_pool();
    let old = EquipmentKeyOpener::new(ring(OLD, false));
    let wrapped = old.generate_wrapped(SHARED).expect("生成比较钥");
    seed_replay(pool, SHARED, &wrapped, false).await;
    let first_scan = scan(pool).await;
    let second_scan = scan(second_pool).await;
    let rotated = EquipmentKeyOpener::new(ring(CURRENT, true));
    let left = rotated_replay(&rotated.rewrap(SHARED, &wrapped).expect("第一份新包装"));
    let right = rotated_replay(&rotated.rewrap(SHARED, &wrapped).expect("第二份新包装"));
    let mut lock = pool.begin().await.expect("同步事务");
    sqlx::query("SELECT id FROM public.consignment_replay_keys WHERE id = $1 FOR UPDATE")
        .bind(SHARED)
        .execute(&mut *lock)
        .await
        .expect("先锁行");

    let release = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let waiters: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM pg_stat_activity
                     WHERE datname = current_database() AND wait_event_type = 'Lock'
                       AND query LIKE 'UPDATE public.consignment_replay_keys%'",
                )
                .fetch_one(pool)
                .await
                .expect("观察两个独立写入者");
                if waiters == 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("两次 CAS 确实同时等待同一行");
        lock.commit().await.expect("一并放行竞争");
    };
    let (first, second, ()) = tokio::join!(
        replace_key_envelope(pool, &first_scan[0], &left),
        replace_key_envelope(second_pool, &second_scan[0], &right),
        release
    );
    let first = first.expect("第一个轮换结果");
    let second = second.expect("第二个轮换结果");
    assert_ne!(first.was_replaced(), second.was_replaced());
    let winner = first
        .current_envelope()
        .or_else(|| second.current_envelope())
        .expect("输家重新读取赢家包装");
    rotated.check_wrapped(SHARED, &wrapped_row(winner)).expect("赢家包装可认证");
    assert!(scan(pool).await.is_empty());
    second_pool.close().await;
    pool.close().await;
}

#[tokio::test]
async fn equipment_rotation_rewrap_preserves_the_retired_logical_key_and_command_mac() {
    let database = TestDatabase::create().await.expect("克隆库");
    let store = database.store().await.expect("连接临时库");
    let pool = store.postgres_pool();
    let old_wrapped = seed_three_sources(pool).await;
    let old = EquipmentKeyOpener::new(ring(OLD, false));
    let keyring = ring(CURRENT, true);
    let rotating = EquipmentKeyOpener::new(Arc::clone(&keyring));
    let current_only = EquipmentKeyOpener::new(ring(CURRENT, false));
    let sealer = EquipmentKeySealer::new(Arc::clone(keyring.current()));
    let identity = ListingEquipmentIdentity::new("main", "73").expect("规范装备身份");
    let original = fake_key();
    let before = old
        .registration_mac(SHARED, &old_wrapped, &input(&identity, &original))
        .expect("旧钥可重放");

    sqlx::query("UPDATE public.consignment_replay_keys SET write_current = false WHERE id = $1")
        .bind(SHARED)
        .execute(pool)
        .await
        .expect("逻辑钥降为只校验");
    let new_wrapped = current_only.generate_wrapped(SECOND).expect("新一代逻辑钥");
    seed_replay(pool, SECOND, &new_wrapped, true).await;

    let page = scan(pool).await;
    assert_eq!(page.len(), 3);
    for row in page {
        let replacement = if let Some(replay_key_id) = row.replay_key_id() {
            rotated_replay(&rotating.rewrap(replay_key_id, &wrapped_row(&row)).expect("重包逻辑钥"))
        } else {
            let aad = EquipmentId::from_uuid(row.equipment_aad().expect("凭证认证身份"));
            let envelope = EquipmentKeyEnvelope::new(
                row.ciphertext.clone(),
                row.nonce.clone(),
                row.encryption_key_id.clone(),
                row.fingerprint.clone(),
            )
            .expect("信封有效");
            let secret = rotating.open(aad, &envelope).expect("开封原凭证");
            rotated_equipment(&sealer.seal(aad, &secret).expect("重封凭证"))
        };
        assert!(replace_key_envelope(pool, &row, &replacement).await.expect("写回").was_replaced());
    }
    assert!(
        stored_key_ids(pool, DataKeyDomain::Equipment).await.expect("旧引用清零")
            == [(CURRENT.to_owned(), 4)]
    );

    let page =
        StoredKeyEnvelope::scan_equipment_for_verification(pool, None, 10).await.expect("全域复核");
    let registration = &page[1];
    let envelope = EquipmentKeyEnvelope::new(
        registration.ciphertext.clone(),
        registration.nonce.clone(),
        registration.encryption_key_id.clone(),
        registration.fingerprint.clone(),
    )
    .expect("新信封结构合法");
    let secret = ListingRegistrationSecret::new(
        EquipmentId::from_uuid(registration.equipment_aad().expect("原 AAD 身份")),
        envelope,
    )
    .expect("活动登记信封");
    assert!(ListingEquipmentKeyOpener::open(&current_only, &secret).is_ok());
    assert!(ListingEquipmentKeyOpener::open(&old, &secret).is_err());
    retire_registration(pool).await;

    let ledger = sqlx::query(
        "SELECT c.request_digest, c.replay_key_id, k.write_current
         FROM public.consignment_commands c JOIN public.consignment_replay_keys k ON k.id = c.replay_key_id
         WHERE c.command_id = $1",
    )
    .bind(COMMAND).fetch_one(pool).await.expect("读取旧登记终局");
    assert_eq!(ledger.get::<Uuid, _>("replay_key_id"), SHARED);
    assert!(!ledger.get::<bool, _>("write_current"));
    let stored: Vec<u8> = ledger.get("request_digest");
    let stored = RegistrationRequestMac::new(stored.try_into().expect("账本摘要长度"));
    assert!(stored.matches(&before));
    let rewrapped = wrapped_row(&page[2]);
    let replay = current_only
        .registration_mac(SHARED, &rewrapped, &input(&identity, &original))
        .expect("仅用新数据钥重放");
    assert!(stored.matches(&replay));
    let changed = EquipmentKey::new("bbccddeeff").expect("另一假凭证");
    let conflict = current_only
        .registration_mac(SHARED, &rewrapped, &input(&identity, &changed))
        .expect("不同请求摘要");
    assert!(!stored.matches(&conflict));
    assert!(old.check_wrapped(SHARED, &rewrapped).is_err());
    assert!(current_only.check_wrapped(SHARED, &old_wrapped).is_err());
    let owner = database.maintenance_store().await.expect("维护账号验证外键");
    let error = sqlx::query("DELETE FROM public.consignment_replay_keys WHERE id = $1")
        .bind(SHARED)
        .execute(owner.postgres_pool())
        .await
        .expect_err("账本引用的逻辑钥不能删除");
    let code = error.as_database_error().and_then(sqlx::error::DatabaseError::code);
    // PG18 为 ON DELETE RESTRICT 返回独立的 restrict_violation。
    assert_eq!(code.as_deref(), Some("23001"), "引用完整性错误只记录 SQLSTATE");
    owner.postgres_pool().close().await;
    pool.close().await;
}
