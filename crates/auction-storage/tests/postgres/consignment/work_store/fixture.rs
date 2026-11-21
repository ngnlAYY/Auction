//! 使用临时 PG18 与真实运行账号的寄售工作夹具。

use auction_application::consignment::{
    AutoCloseCandidate, AutoCloseListingCommand, ListingLookupBudget, ListingLookupClaim,
    LookupClaimRequest, LookupFence, LookupWorkStore,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::support::{TestDatabase, fixtures};

pub(super) const SELLER: Uuid = Uuid::from_u128(0x2f01);
pub(super) const LISTING: Uuid = Uuid::from_u128(0x2f02);
pub(super) const WORK: Uuid = Uuid::from_u128(0x2f03);
pub(super) const OTHER_LISTING: Uuid = Uuid::from_u128(0x2f04);
pub(super) const OTHER_WORK: Uuid = Uuid::from_u128(0x2f05);
pub(super) const REGISTERED_AAD: Uuid = Uuid::from_u128(0x2f08);
const OTHER_AAD: Uuid = Uuid::from_u128(0x2f09);
const REPLAY_KEY: Uuid = Uuid::from_u128(0x2f06);
const TEST_TOKEN: Uuid = Uuid::from_u128(0x2f07);

pub(super) struct Fixture {
    pub(super) db: DatabaseStore,
    pub(super) owner: DatabaseStore,
    _database: TestDatabase,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        Self::deadline_seconds(3600.0).await
    }

    pub(super) async fn deadline_seconds(seconds: f64) -> Self {
        let database = TestDatabase::create().await.unwrap();
        let owner = database.maintenance_store().await.unwrap();
        let db = database.store().await.unwrap();
        let pool = owner.postgres_pool();
        fixtures::seed_user(pool, SELLER, "FakeWorkStoreSeller").await;
        let mut tx = pool.begin().await.unwrap();
        let (created, expires): (Timestamp, Timestamp) = sqlx::query_as(
            "SELECT statement_timestamp() - interval '24 hours',
                    statement_timestamp() + make_interval(secs => $1)",
        )
        .bind(seconds)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO public.consignment_replay_keys
             (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
             VALUES ($1, true, decode(repeat('11', 48), 'hex'), decode(repeat('22', 12), 'hex'),
                     'fake-replay-key', $2, $2)",
        )
        .bind(REPLAY_KEY)
        .bind(created)
        .execute(&mut *tx)
        .await
        .unwrap();
        for (listing, work, eid, aad) in
            [(LISTING, WORK, "941", REGISTERED_AAD), (OTHER_LISTING, OTHER_WORK, "942", OTHER_AAD)]
        {
            seed_listing(&mut tx, listing, work, eid, aad, created, expires).await;
        }
        tx.commit().await.unwrap();
        Self { db, owner, _database: database }
    }

    pub(super) fn pool(&self) -> &PgPool {
        self.db.postgres_pool()
    }

    pub(super) fn work(&self) -> impl LookupWorkStore + use<> {
        self.db.consignment_lookup(deadline_command, exhausted_command)
    }

    pub(super) async fn claim(&self, epoch: u64) -> Option<ListingLookupClaim> {
        self.work().claim(request(epoch, 60_000)).await.unwrap()
    }

    pub(super) async fn expires(&self) -> Timestamp {
        sqlx::query_scalar("SELECT expires_at FROM public.consignment_listings WHERE id = $1")
            .bind(LISTING)
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    pub(super) async fn allow_now(&self) {
        sqlx::query("UPDATE public.consignment_lookup_control SET next_allowed_at = statement_timestamp() WHERE id = 1")
            .execute(self.pool()).await.unwrap();
    }

    pub(super) async fn due_now(&self) {
        sqlx::query("UPDATE public.consignment_lookup_work SET next_attempt_at = created_at - interval '1 second' WHERE id = $1")
            .bind(WORK).execute(self.pool()).await.unwrap();
        self.allow_now().await;
    }

    pub(super) async fn expire_lease(&self, fence: LookupFence) {
        // 推进这次工作和许可的数据库期限，模拟崩溃后到期，测试无需等待真实退避。
        sqlx::query(
            "WITH changed AS (
                 UPDATE public.consignment_lookup_work SET
                     lease_until = statement_timestamp() - interval '1 second',
                     network_deadline = statement_timestamp() - interval '6 seconds'
                 WHERE id = $1 RETURNING lease_until
             )
             UPDATE public.consignment_lookup_control SET permit_until = changed.lease_until
             FROM changed WHERE holder_work_id = $1",
        )
        .bind(fence.work_id())
        .execute(self.pool())
        .await
        .unwrap();
    }

    pub(super) async fn seed_lease(&self, attempt: i16, expired: bool) -> LookupFence {
        sqlx::query(
            "WITH lease AS (
                UPDATE public.consignment_lookup_work SET state = 'leased', lease_owner = 'fake-worker',
                    token = $2, fence = $3, attempt_count = $3, next_attempt_at = NULL,
                    network_deadline = statement_timestamp() + make_interval(secs => $4),
                    lease_until = statement_timestamp() + make_interval(secs => $5)
                WHERE id = $1 RETURNING fence, lease_until
             )
             UPDATE public.consignment_lookup_control
             SET holder_work_id = $1, holder_token = $2, holder_fence = lease.fence,
                 permit_until = lease.lease_until FROM lease WHERE id = 1",
        )
        .bind(WORK)
        .bind(TEST_TOKEN)
        .bind(attempt)
        .bind(if expired { -6.0f64 } else { 20.0 })
        .bind(if expired { -1.0f64 } else { 25.0 })
        .execute(self.pool())
        .await
        .unwrap();
        LookupFence::new(WORK, LISTING, 1, i64::from(attempt), TEST_TOKEN).unwrap()
    }

    pub(super) async fn expire_other_lease(&self) {
        sqlx::query(
            "UPDATE public.consignment_lookup_work SET state = 'leased', lease_owner = 'fake-worker',
                 token = $2, fence = 1, attempt_count = 1, next_attempt_at = NULL,
                 network_deadline = statement_timestamp() - interval '6 seconds',
                 lease_until = statement_timestamp() - interval '1 second' WHERE id = $1",
        )
        .bind(OTHER_WORK)
        .bind(TEST_TOKEN)
        .execute(self.pool())
        .await
        .unwrap();
    }

    pub(super) async fn seed_exhausted_backlog(&self, count: u128) {
        let mut tx = self.owner.postgres_pool().begin().await.unwrap();
        let (created, expires) = sqlx::query_as(
            "SELECT created_at, expires_at FROM public.consignment_listings WHERE id = $1",
        )
        .bind(LISTING)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        for offset in 1..count {
            seed_listing(
                &mut tx,
                Uuid::from_u128(0x3000 + offset),
                Uuid::from_u128(0x3100 + offset),
                &(950 + offset).to_string(),
                Uuid::from_u128(0x3200 + offset),
                created,
                expires,
            )
            .await;
        }
        sqlx::query(
            "UPDATE public.consignment_lookup_work w SET state = 'leased', lease_owner = 'fake-worker',
                 token = $2, fence = 6, attempt_count = 6, next_attempt_at = NULL,
                 network_deadline = healthy.lease_until - interval '15 seconds',
                 lease_until = healthy.lease_until - interval '10 seconds'
             FROM public.consignment_lookup_work healthy WHERE healthy.id = $1 AND w.id <> $1",
        )
        .bind(OTHER_WORK)
        .bind(TEST_TOKEN)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    pub(super) async fn attempts(&self) -> i64 {
        sqlx::query_scalar("SELECT sum(attempt_count)::bigint FROM public.consignment_lookup_work")
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    pub(super) async fn heartbeat(&self) -> Option<Timestamp> {
        sqlx::query_scalar("SELECT worker_ready_until FROM public.consignment_lookup_control")
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    pub(super) async fn work_snapshot(&self) -> String {
        // xmin 也参与比较，能识别把字段写回旧值的伪零写入；诊断不输出凭据或快照内容。
        sqlx::query_scalar(
            "SELECT md5((SELECT jsonb_agg(to_jsonb(w) || jsonb_build_object('xmin', xmin::text) ORDER BY id)
                         FROM public.consignment_lookup_work w)::text ||
                        (SELECT (to_jsonb(c) || jsonb_build_object('xmin', xmin::text))::text
                         FROM public.consignment_lookup_control c WHERE id = 1))",
        )
        .fetch_one(self.pool())
        .await
        .unwrap()
    }
}

pub(super) fn request(epoch: u64, interval_ms: u64) -> LookupClaimRequest {
    LookupClaimRequest::new(
        epoch,
        "fake-worker".into(),
        Uuid::new_v4(),
        ListingLookupBudget::new(30, interval_ms).unwrap(),
    )
    .unwrap()
}

pub(super) fn deadline_command(candidate: AutoCloseCandidate) -> AutoCloseListingCommand {
    // 确定性应用接线替身。断言仓储原样使用调用方构造的 ID 与摘要。
    let command_id = Uuid::new_v5(&candidate.listing_id, b"fake-deadline-command");
    AutoCloseListingCommand {
        request_hash: RequestHash::new([0xa1; 32]),
        command_id: CommandId::from_uuid(command_id),
        candidate,
    }
}

pub(super) fn exhausted_command(fence: LookupFence) -> (CommandId, RequestHash) {
    let command_id = Uuid::new_v5(&fence.work_id(), b"fake-reaper-exhausted-command");
    (CommandId::from_uuid(command_id), RequestHash::new([0xa2; 32]))
}

async fn seed_listing(
    tx: &mut Transaction<'_, Postgres>,
    listing: Uuid,
    work: Uuid,
    eid: &str,
    aad: Uuid,
    created: Timestamp,
    expires: Timestamp,
) {
    let command = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.consignment_listings
         (id, seller_user_id, world_id, external_equipment_id, mode, state, price, pricing_version,
          fee_bps, state_version, service_epoch, blacklist_count_at_register, created_at, updated_at, expires_at)
         VALUES ($1, $2, 'main', $3, 'direct', 'resolving', 101, 1, 0, 1, 1, 0, $4, $4, $5)",
    )
    .bind(listing).bind(SELLER).bind(eid).bind(created).bind(expires)
    .execute(&mut **tx).await.unwrap();
    sqlx::query(
        "INSERT INTO public.consignment_commands
         (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
          replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ($1, 'public_user', $2::uuid::text, 'register', 'new', decode(repeat('11', 32), 'hex'),
                 $3, $4, 202, 'accepted',
                 jsonb_build_object('command_id', $1::uuid, 'operation', 'register',
                     'listing_id', $4::uuid, 'listing_version', 1, 'state', 'resolving',
                     'disposition', 'registered', 'price', 101), $5)",
    )
    .bind(command).bind(SELLER).bind(REPLAY_KEY).bind(listing).bind(created)
    .execute(&mut **tx).await.unwrap();
    sqlx::query(
        "INSERT INTO public.consignment_events
         (listing_id, state_version, command_id, actor_kind, event_kind, from_state, to_state,
          reason_code, price, occurred_at)
         VALUES ($1, 1, $2, 'public_user', 'registered', NULL, 'resolving', NULL, 101, $3)",
    )
    .bind(listing)
    .bind(command)
    .bind(created)
    .execute(&mut **tx)
    .await
    .unwrap();
    let ciphertext_byte = if aad == REGISTERED_AAD { "33" } else { "55" };
    sqlx::query(
        "INSERT INTO public.consignment_registration_secrets
         (listing_id, aad_equipment_id, ciphertext, nonce, encryption_key_id, fingerprint, created_at, updated_at)
         VALUES ($1, $2, decode(repeat($4, 32), 'hex'), decode(repeat('44', 12), 'hex'),
                 'fake-data-key', repeat('ab', 32), $3, $3)",
    )
    .bind(listing).bind(aad).bind(created).bind(ciphertext_byte).execute(&mut **tx).await.unwrap();
    sqlx::query(
        "INSERT INTO public.consignment_lookup_work
         (id, listing_id, generation, state, attempt_count, fence, next_attempt_at, created_at, updated_at)
         VALUES ($1, $2, 1, 'queued', 0, 0, $3, $3, $3)",
    )
    .bind(work).bind(listing).bind(created).execute(&mut **tx).await.unwrap();
}
