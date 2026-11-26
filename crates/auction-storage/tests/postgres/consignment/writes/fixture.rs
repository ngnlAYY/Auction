//! 六类命令测试的本地凭证替身、端口输入与持久事实观测。

use std::sync::Mutex;
use std::time::Duration;

use auction_application::admin::{AdminIntakeError, EquipmentKeySealerPort};
use auction_application::consignment::{
    AdminDelistListingCommand, ApplyListingLookupCommand, AutoCloseCandidate, AutoCloseEvidence,
    AutoCloseListingCommand, ListingCommandStore, ListingCredentialError, ListingDelistReason,
    ListingEquipmentDetails, ListingEquipmentLabel, ListingEquipmentLevel, ListingLookupSnapshot,
    ListingRegistrationPolicy, ListingWorlds, LookupFence, LookupObservation,
    RegisterListingCommand, RegistrationInput, RegistrationReplayCrypto, RegistrationRequestMac,
    RepriceListingCommand, SellerListingTarget, WithdrawListingCommand,
    WrappedRegistrationReplayKey,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::{
    FeeBps, ListingEquipmentIdentity, ListingMode, ListingPrice, ListingPublicationTtls,
    ListingVersion,
};
use auction_domain::{CommandId, EquipmentId, EquipmentKey, EquipmentKeyEnvelope, UserId};
use auction_storage::DatabaseStore;
use auction_time::{Duration as TimeDelta, Timestamp};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support::{TestDatabase, fixtures};

pub(super) const SELLER: Uuid = Uuid::from_u128(10);
pub(super) const OTHER: Uuid = Uuid::from_u128(20);
pub(super) const FAKE_KEY: &str = "01234abcde";
pub(super) const OTHER_FAKE_KEY: &str = "01234abcdf";

#[derive(Default)]
pub(super) struct Crypto {
    pub(super) fail_mac: bool,
    pub(super) fail_seal: bool,
    pub(super) seals: Mutex<Vec<EquipmentId>>,
}

impl EquipmentKeySealerPort for Crypto {
    fn seal(
        &self,
        equipment_id: EquipmentId,
        _key: &EquipmentKey,
    ) -> Result<EquipmentKeyEnvelope, AdminIntakeError> {
        self.seals.lock().unwrap().push(equipment_id);
        if self.fail_seal {
            return Err(AdminIntakeError::KeyUnavailable);
        }
        Ok(EquipmentKeyEnvelope::new(
            vec![0xa1; 28],
            vec![0xb2; 12],
            "fake-data-key".into(),
            "ab".repeat(32),
        )
        .unwrap())
    }
}

impl RegistrationReplayCrypto for Crypto {
    fn generate_wrapped(
        &self,
        _key_id: Uuid,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        WrappedRegistrationReplayKey::new(vec![0xc3; 48], [0xd4; 12], "fake-data-key".into())
    }

    fn check_wrapped(
        &self,
        _key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<(), ListingCredentialError> {
        if wrapped.wrapped_key() == [0xc3; 48] && wrapped.nonce() == &[0xd4; 12] {
            Ok(())
        } else {
            Err(ListingCredentialError)
        }
    }

    fn registration_mac(
        &self,
        key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
        input: &RegistrationInput<'_>,
    ) -> Result<RegistrationRequestMac, ListingCredentialError> {
        self.check_wrapped(key_id, wrapped)?;
        if self.fail_mac {
            return Err(ListingCredentialError);
        }
        // 故意只支持本文件的两个假值，返回固定测试标记的摘要；真实 HMAC 由 2C 验证。
        let marker = input
            .key
            .with_bytes(|key| {
                if key == FAKE_KEY.as_bytes() {
                    1_u8
                } else if key == OTHER_FAKE_KEY.as_bytes() {
                    2
                } else {
                    0
                }
            })
            .map_err(|_error| ListingCredentialError)?;
        if marker == 0 {
            return Err(ListingCredentialError);
        }
        Ok(RegistrationRequestMac::new(
            digest(&[
                key_id.as_bytes(),
                input.identity.world_id().as_bytes(),
                input.identity.external_equipment_id().as_bytes(),
                input.mode.as_str().as_bytes(),
                &input.price.value().to_be_bytes(),
                &input.expected_fee_bps.value().to_be_bytes(),
                &[marker],
            ])
            .as_bytes(),
        ))
    }

    fn rewrap(
        &self,
        key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        self.check_wrapped(key_id, wrapped)?;
        Ok(wrapped.clone())
    }
}

pub(super) struct Fixture {
    pub(super) store: DatabaseStore,
    pub(super) owner: DatabaseStore,
    pub(super) crypto: Crypto,
    pub(super) key: EquipmentKey,
    _db: TestDatabase,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        let db = TestDatabase::create().await.unwrap();
        let owner = db.maintenance_store().await.unwrap();
        let store = db.store().await.unwrap();
        fixtures::seed_user(owner.postgres_pool(), SELLER, "FakeWriteSeller").await;
        fixtures::seed_user(owner.postgres_pool(), OTHER, "FakeWriteOther").await;
        sqlx::query(
            "UPDATE public.consignment_lookup_control
             SET admitting = true, worker_ready_until = statement_timestamp() + interval '10 minutes'",
        ).execute(owner.postgres_pool()).await.unwrap();
        Self {
            store,
            owner,
            crypto: Crypto::default(),
            key: EquipmentKey::new(FAKE_KEY).unwrap(),
            _db: db,
        }
    }

    pub(super) fn pool(&self) -> &PgPool {
        self.store.postgres_pool()
    }

    pub(super) async fn registered(
        &self,
        listing_id: Uuid,
        gear: &ListingEquipmentIdentity,
        seller: Uuid,
        mode: ListingMode,
    ) -> Uuid {
        let cmd = register(new_id(), listing_id, gear, &self.key, seller, mode);
        let work_id = cmd.work_id;
        let receipt = self
            .store
            .register(cmd, Some(&self.crypto), Some(&self.crypto))
            .await
            .unwrap()
            .into_result()
            .unwrap();
        assert_eq!(receipt.listing_id(), listing_id);
        work_id
    }

    pub(super) async fn grant(
        &self,
        listing_id: Uuid,
        work_id: Uuid,
        attempt: i16,
        seconds: i32,
    ) -> LookupFence {
        let token = new_id();
        let mut tx = self.pool().begin().await.unwrap();
        DatabaseStore::test_lookup_lock(&mut tx).await.unwrap();
        let lease: Timestamp = sqlx::query_scalar(
            "WITH tick AS (SELECT statement_timestamp() + make_interval(secs => $4) AS at)
             UPDATE public.consignment_lookup_work
             SET state='leased', attempt_count=$2, fence=$2, token=$3, lease_owner='fake-worker',
                 lease_until=tick.at, network_deadline=tick.at - interval '1 second',
                 next_attempt_at=NULL, completed_at=NULL
             FROM tick WHERE id=$1 RETURNING lease_until",
        )
        .bind(work_id)
        .bind(attempt)
        .bind(token)
        .bind(f64::from(seconds))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE public.consignment_lookup_control SET holder_work_id=$1, holder_fence=$2,
                 holder_token=$3, permit_until=$4 WHERE id=1",
        )
        .bind(work_id)
        .bind(i64::from(attempt))
        .bind(token)
        .bind(lease)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        LookupFence::new(work_id, listing_id, 1, i64::from(attempt), token).unwrap()
    }

    pub(super) async fn state(&self, listing_id: Uuid) -> (String, i64, Option<String>, bool) {
        sqlx::query_as(
            "SELECT state, state_version, reason_code, current_snapshot_id IS NOT NULL
             FROM public.consignment_listings WHERE id=$1",
        )
        .bind(listing_id)
        .fetch_one(self.pool())
        .await
        .unwrap()
    }

    pub(super) async fn retired(&self, listing_id: Uuid) -> bool {
        sqlx::query_scalar(
            "SELECT num_nonnulls(ciphertext, nonce, encryption_key_id, fingerprint) = 0
             AND retired_at IS NOT NULL AND retired_at = updated_at
             FROM public.consignment_registration_secrets WHERE listing_id=$1",
        )
        .bind(listing_id)
        .fetch_one(self.pool())
        .await
        .unwrap()
    }

    pub(super) async fn deadline(&self, listing_id: Uuid) -> Timestamp {
        sqlx::query_scalar("SELECT expires_at FROM public.consignment_listings WHERE id=$1")
            .bind(listing_id)
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    pub(super) async fn persisted(&self) -> Value {
        sqlx::query_scalar(
            "SELECT jsonb_build_array(
               (SELECT jsonb_agg(to_jsonb(l) ORDER BY id) FROM public.consignment_listings l),
               (SELECT jsonb_agg(to_jsonb(w) ORDER BY id) FROM public.consignment_lookup_work w),
               (SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM public.consignment_lookup_control c),
               (SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM public.consignment_snapshots s),
               (SELECT jsonb_agg(to_jsonb(s) ORDER BY listing_id) FROM public.consignment_registration_secrets s),
               (SELECT jsonb_agg(to_jsonb(c) ORDER BY command_id) FROM public.consignment_commands c),
               (SELECT jsonb_agg(to_jsonb(k) ORDER BY id) FROM public.consignment_replay_keys k),
               (SELECT jsonb_agg(to_jsonb(e) ORDER BY listing_id, state_version) FROM public.consignment_events e),
               (SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM public.audit_records a WHERE action LIKE 'consignment.%'),
               (SELECT jsonb_agg(to_jsonb(r) ORDER BY listing_id, lot_id) FROM public.consignment_listing_auction_conflicts r),
               (SELECT jsonb_agg(to_jsonb(b) ORDER BY listing_id, lot_id) FROM public.consignment_listing_lot_baselines b)
             )",
        ).fetch_one(self.pool()).await.unwrap()
    }

    pub(super) async fn audit(&self, command_id: Uuid) -> Vec<(String, String, String)> {
        sqlx::query_as(
            "SELECT actor_type, action, result FROM public.audit_records
             WHERE metadata->>'command_id' = $1 ORDER BY action",
        )
        .bind(command_id.to_string())
        .fetch_all(self.pool())
        .await
        .unwrap()
    }

    pub(super) async fn command_count(&self, command_id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM public.consignment_commands WHERE command_id=$1")
            .bind(command_id)
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    pub(super) async fn block_control(&self) -> sqlx::Transaction<'static, sqlx::Postgres> {
        let mut tx = self.owner.postgres_pool().begin().await.unwrap();
        DatabaseStore::test_lookup_lock(&mut tx).await.unwrap();
        tx
    }

    pub(super) async fn release_when_waiting(
        &self,
        tx: sqlx::Transaction<'static, sqlx::Postgres>,
        waiting: i64,
    ) {
        let reached = fixtures::wait_for_condition(Duration::from_millis(850), || async {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks l JOIN pg_stat_activity p USING (pid)
                 WHERE NOT l.granted AND p.datname = current_database()",
            )
            .fetch_one(self.owner.postgres_pool())
            .await
            .unwrap();
            count >= waiting
        })
        .await;
        tx.commit().await.unwrap();
        assert!(reached, "并发连接须到达明确锁等待点");
    }

    pub(super) async fn lot(&self, eid: &str, sequence: i64) -> Uuid {
        let lot_id = new_id();
        self.lot_with_id(eid, sequence, lot_id).await;
        lot_id
    }

    pub(super) async fn lot_with_id(&self, eid: &str, sequence: i64, lot_id: Uuid) {
        let mut tx = self.owner.postgres_pool().begin().await.unwrap();
        let equipment_id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO public.equipment (id,world_id,external_equipment_id,name,category,collected_at)
             VALUES (gen_random_uuid(),'main',$1,'Fake Equipment','Weapon',statement_timestamp())
             ON CONFLICT (world_id,external_equipment_id) DO UPDATE SET name = EXCLUDED.name
             RETURNING id",
        ).bind(eid).fetch_one(&mut *tx).await.unwrap();
        let when = "2026-01-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let auction_id = new_id();
        sqlx::query(
            "INSERT INTO public.auctions
                (id,world_id,sequence,title,status,scheduled_open_at,preview_scheduled_at,scheduled_end_at,
                 effective_end_at,preview_opened_at,opened_at,policy_snapshot)
             VALUES ($1,'main',$2,'Fake Auction','settled',$3,$3,$3,$3,$3,$3,$4)",
        ).bind(auction_id).bind(sequence).bind(when).bind(fixtures::policy_snapshot(when, 50_000, 30_000))
            .execute(&mut *tx).await.unwrap();
        sqlx::query(
            "INSERT INTO public.lots
                (id,auction_id,equipment_id,lot_number,status,start_price,category_key,category_number,created_at,updated_at)
             VALUES ($1,$2,$3,1,'unsold',50000,'Weapon',1,$4,$4)",
        ).bind(lot_id).bind(auction_id).bind(equipment_id).bind(when).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }

    pub(super) async fn set_blacklist(&self, seller: Uuid) -> Uuid {
        let mut tx = self.owner.postgres_pool().begin().await.unwrap();
        sqlx::query("UPDATE public.users SET status='blacklisted' WHERE id=$1")
            .bind(seller)
            .execute(&mut *tx)
            .await
            .unwrap();
        let fact = new_id();
        sqlx::query(
            "INSERT INTO public.user_blacklist_history (id,user_id,public_reason,actor,blacklisted_at)
             VALUES ($1,$2,'测试资格关闭','fake-audit',statement_timestamp())",
        ).bind(fact).bind(seller).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        fact
    }
}

pub(super) fn new_id() -> Uuid {
    Uuid::new_v4()
}

pub(super) fn new_internal_command_id() -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, new_id().as_bytes())
}

pub(super) fn gear(world: &str, eid: &str) -> ListingEquipmentIdentity {
    ListingEquipmentIdentity::new(world, eid).unwrap()
}

pub(super) fn price(amount: i64) -> ListingPrice {
    ListingPrice::new(amount).unwrap()
}

pub(super) fn register<'a>(
    command_id: Uuid,
    listing_id: Uuid,
    equipment: &'a ListingEquipmentIdentity,
    key: &'a EquipmentKey,
    seller_user_id: Uuid,
    mode: ListingMode,
) -> RegisterListingCommand<'a> {
    RegisterListingCommand {
        request_id: None,
        command_id: CommandId::from_uuid(command_id),
        input: RegistrationInput {
            seller: auction_application::consignment::RegistrationSeller::Account(
                UserId::from_uuid(seller_user_id),
            ),
            identity: equipment,
            key,
            mode,
            price: price(101),
            expected_fee_bps: FeeBps::for_mode(mode, FeeBps::DEFAULT_CONSIGNED),
        },
        listing_id,
        work_id: new_id(),
        aad_equipment_id: EquipmentId::from_uuid(new_id()),
        candidate_replay_key_id: new_id(),
        policy: ListingRegistrationPolicy {
            enabled: true,
            worlds: ListingWorlds { main: true, isekai: true },
            consigned_fee_bps: FeeBps::DEFAULT_CONSIGNED,
            max_open_per_user: 20,
            registrations_per_hour: 10,
            anonymous_resolving_limit: 50,
        },
    }
}

pub(super) fn digest(parts: &[&[u8]]) -> RequestHash {
    // 端口测试只验证原证明的相等性；应用层另有规范编码契约测试。
    RequestHash::from_canonical_bytes(&parts.concat())
}

fn seller_target(
    command_id: Uuid,
    listing_id: Uuid,
    seller: Uuid,
    version: u64,
) -> SellerListingTarget {
    SellerListingTarget {
        command_id: CommandId::from_uuid(command_id),
        listing_id,
        seller_user_id: UserId::from_uuid(seller),
        expected_version: ListingVersion::new(version).unwrap(),
    }
}

pub(super) fn reprice(
    command_id: Uuid,
    listing_id: Uuid,
    seller: Uuid,
    version: u64,
    new_price: i64,
) -> RepriceListingCommand {
    RepriceListingCommand {
        request_id: None,
        request_hash: digest(&[
            listing_id.as_bytes(),
            &version.to_be_bytes(),
            &new_price.to_be_bytes(),
        ]),
        target: seller_target(command_id, listing_id, seller, version),
        price: price(new_price),
        enabled: true,
    }
}

pub(super) fn withdraw(
    command_id: Uuid,
    listing_id: Uuid,
    seller: Uuid,
    version: u64,
) -> WithdrawListingCommand {
    WithdrawListingCommand {
        request_id: None,
        request_hash: digest(&[listing_id.as_bytes(), &version.to_be_bytes()]),
        target: seller_target(command_id, listing_id, seller, version),
    }
}

pub(super) fn delist(
    command_id: Uuid,
    listing_id: Uuid,
    version: u64,
    reason: &str,
) -> AdminDelistListingCommand {
    AdminDelistListingCommand {
        request_id: None,
        admin_actor: "fake-admin".into(),
        request_hash: digest(&[listing_id.as_bytes(), &version.to_be_bytes(), reason.as_bytes()]),
        command_id: CommandId::from_uuid(command_id),
        listing_id,
        expected_version: ListingVersion::new(version).unwrap(),
        reason: ListingDelistReason::new(reason.into()).unwrap(),
    }
}

pub(super) fn auto(
    command_id: Uuid,
    listing_id: Uuid,
    evidence: AutoCloseEvidence,
) -> AutoCloseListingCommand {
    let mut bytes = listing_id.as_bytes().to_vec();
    bytes.extend(format!("{evidence:?}").as_bytes());
    AutoCloseListingCommand {
        request_hash: RequestHash::from_canonical_bytes(&bytes),
        command_id: CommandId::from_uuid(command_id),
        candidate: AutoCloseCandidate { listing_id, evidence },
    }
}

pub(super) fn observation(
    equipment: &ListingEquipmentIdentity,
    key: &EquipmentKey,
    owner: &str,
    tradeable: bool,
) -> LookupObservation {
    let details = ListingEquipmentDetails::new(
        "2026-01-02T00:00:00Z".parse().unwrap(),
        ListingEquipmentLabel {
            name: "Fake Longsword".into(),
            alt_name: None,
            category: "Weapon".into(),
            level: if tradeable {
                ListingEquipmentLevel::Assigned(100)
            } else {
                ListingEquipmentLevel::Soulbound
            },
            tradeable,
        },
        Vec::new(),
    )
    .unwrap();
    LookupObservation::Equipment(
        ListingLookupSnapshot::new(equipment.clone(), details, owner.into(), key).unwrap(),
    )
}

pub(super) fn apply(
    command_id: Uuid,
    fence: LookupFence,
    observation: LookupObservation,
    variant: u8,
) -> ApplyListingLookupCommand {
    ApplyListingLookupCommand {
        request_hash: digest(&[fence.work_id().as_bytes(), fence.token().as_bytes(), &[variant]]),
        command_id: CommandId::from_uuid(command_id),
        fence,
        observation,
        publication_ttls: ListingPublicationTtls::default(),
    }
}

pub(super) async fn expire_listing(fixture: &Fixture, listing_id: Uuid) -> Timestamp {
    // 只倒移测试截止；生产触发器仍会验证被测命令的预计版本、到期守卫、快照和同事务事件。
    let mut tx = fixture.owner.postgres_pool().begin().await.unwrap();
    sqlx::query("ALTER TABLE public.consignment_listings DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .unwrap();
    let expires_at = sqlx::query_scalar(
        "WITH tick AS (SELECT statement_timestamp() - interval '1 second' AS at)
         UPDATE public.consignment_listings SET created_at=LEAST(created_at,tick.at - interval '1 millisecond'),
             expires_at=tick.at FROM tick WHERE id=$1 RETURNING expires_at",
    ).bind(listing_id).fetch_one(&mut *tx).await.unwrap();
    sqlx::query("ALTER TABLE public.consignment_listings ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    expires_at
}

pub(super) fn time_delta(seconds: i64) -> TimeDelta {
    TimeDelta::seconds(seconds)
}
