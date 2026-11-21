//! PostgreSQL 18 上的解析工作、全局许可与时钟边界。

use std::time::Duration;

use auction_application::consignment::{
    LookupFence, LookupLeaseUpdate, LookupRetryAfter, LookupTransientReason,
};
use auction_storage::DatabaseStore;
use auction_time::{Duration as TimeDelta, Timestamp};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::support::{TestDatabase, fixtures};

const SELLER: Uuid = Uuid::from_u128(0x100);
const LISTING: Uuid = Uuid::from_u128(0x200);
const OTHER_LISTING: Uuid = Uuid::from_u128(0x201);
const WORK: Uuid = Uuid::from_u128(0x300);
const OTHER_WORK: Uuid = Uuid::from_u128(0x301);
const FAKE_TOKEN: Uuid = Uuid::from_u128(0x400);
const OTHER_FAKE_TOKEN: Uuid = Uuid::from_u128(0x401);

struct Fixture {
    store: DatabaseStore,
    _db: TestDatabase,
}

impl Fixture {
    async fn new() -> Self {
        let db = TestDatabase::create().await.expect("创建测试库");
        {
            let owner = db.maintenance_store().await.expect("连接测试管理账号");
            let pool = owner.postgres_pool();
            fixtures::seed_user(pool, SELLER, "FakeLookupSeller").await;
            let mut tx = pool.begin().await.unwrap();
            sqlx::query(
                "INSERT INTO public.consignment_replay_keys
                    (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
                 VALUES ($1, TRUE, decode(repeat('11', 48), 'hex'), decode(repeat('22', 12), 'hex'),
                         'fake-data-key', $2, $2)",
            )
            .bind(Uuid::from_u128(0x500))
            .bind(registered_at())
            .execute(&mut *tx)
            .await
            .unwrap();
            seed_listing(&mut tx, LISTING, "1001", Uuid::from_u128(0x600)).await;
            seed_listing(&mut tx, OTHER_LISTING, "1002", Uuid::from_u128(0x601)).await;
            tx.commit().await.unwrap();
        }
        let store = db.store().await.expect("连接测试运行账号");
        let fixture = Self { store, _db: db };
        sqlx::query(
            "INSERT INTO public.consignment_lookup_work
                 (id, listing_id, generation, state, attempt_count, fence, next_attempt_at,
                  created_at, updated_at)
             VALUES ($1, $2, 1, 'queued', 0, 0, $3, $3, $3),
                    ($4, $5, 1, 'queued', 0, 0, $3, $3, $3)",
        )
        .bind(WORK)
        .bind(LISTING)
        .bind(registered_at())
        .bind(OTHER_WORK)
        .bind(OTHER_LISTING)
        .execute(fixture.pool())
        .await
        .unwrap();
        fixture
    }

    fn pool(&self) -> &PgPool {
        self.store.postgres_pool()
    }

    async fn lock(&self) -> Transaction<'static, Postgres> {
        let mut tx = self.pool().begin().await.unwrap();
        DatabaseStore::test_lookup_lock(&mut tx).await.unwrap();
        tx
    }

    async fn grant(
        &self,
        work_id: Uuid,
        token: Uuid,
        attempt: i16,
        lease_seconds: f64,
    ) -> (Timestamp, Timestamp) {
        let mut tx = self.lock().await;
        let times = sqlx::query_as::<_, (Timestamp, Timestamp)>(
            "WITH times AS (
                 SELECT statement_timestamp() + make_interval(secs => $4) AS lease_until
             )
             UPDATE public.consignment_lookup_work
             SET state = 'leased', attempt_count = $3, fence = 7, token = $2,
                 lease_owner = 'fake-worker', lease_until = times.lease_until,
                 network_deadline = times.lease_until - interval '2 seconds', next_attempt_at = NULL,
                 completed_at = NULL
             FROM times WHERE id = $1
             RETURNING consignment_lookup_work.lease_until, network_deadline",
        )
        .bind(work_id)
        .bind(token)
        .bind(attempt)
        .bind(lease_seconds)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE public.consignment_lookup_control
             SET holder_work_id = $1, holder_fence = 7, holder_token = $2, permit_until = $3
             WHERE id = 1",
        )
        .bind(work_id)
        .bind(token)
        .bind(times.0)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        times
    }

    async fn work(&self, work_id: Uuid) -> Value {
        sqlx::query_scalar(
            "SELECT to_jsonb(w) || jsonb_build_object('xmin', xmin::text)
             FROM public.consignment_lookup_work w WHERE id = $1",
        )
        .bind(work_id)
        .fetch_one(self.pool())
        .await
        .unwrap()
    }

    async fn control(&self) -> Value {
        sqlx::query_scalar(
            "SELECT to_jsonb(c) || jsonb_build_object('xmin', xmin::text)
             FROM public.consignment_lookup_control c WHERE id = 1",
        )
        .fetch_one(self.pool())
        .await
        .unwrap()
    }

    async fn snapshot(&self) -> (Value, Value) {
        (self.work(WORK).await, self.control().await)
    }

    async fn cancel(&self, listing_id: Uuid) -> i64 {
        let mut tx = self.lock().await;
        let count = DatabaseStore::test_lookup_cancel(&mut tx, listing_id).await.unwrap();
        tx.commit().await.unwrap();
        count
    }

    async fn check(
        &self,
        fence: LookupFence,
        at: Option<Timestamp>,
    ) -> Option<LookupLeaseUpdate<i16>> {
        let mut tx = self.lock().await;
        let result = DatabaseStore::test_lookup_fence(&mut tx, fence, at).await.unwrap();
        tx.commit().await.unwrap();
        result
    }

    async fn finish(
        &self,
        fence: LookupFence,
        at: Option<Timestamp>,
        failure: Option<(LookupTransientReason, Option<LookupRetryAfter>)>,
    ) -> LookupLeaseUpdate<Option<Timestamp>> {
        let mut tx = self.lock().await;
        let result = DatabaseStore::test_lookup_finish(&mut tx, fence, at, failure).await.unwrap();
        tx.commit().await.unwrap();
        result
    }
}

fn registered_at() -> Timestamp {
    "2000-01-01T00:00:00Z".parse().unwrap()
}

async fn seed_listing(
    tx: &mut Transaction<'_, Postgres>,
    listing_id: Uuid,
    eid: &str,
    command_id: Uuid,
) {
    let at = registered_at();
    sqlx::query(
        "INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
             created_at, updated_at, expires_at)
         VALUES ($1, $2, 'main', $3, 'direct', 'resolving', 101, 1, 0, 1, 1, 0, $4, $4, $5)",
    )
    .bind(listing_id)
    .bind(SELLER)
    .bind(eid)
    .bind(at)
    .bind(at + TimeDelta::days(1))
    .execute(&mut **tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ($1, 'public_user', $2::uuid::text, 'register', 'new',
                 decode(repeat('11', 32), 'hex'), $3, $4, 202, 'accepted',
                 jsonb_build_object('command_id', $1::uuid, 'operation', 'register', 'listing_id', $4::uuid,
                    'listing_version', 1, 'state', 'resolving', 'disposition', 'registered',
                    'price', 101), $5)",
    )
    .bind(command_id)
    .bind(SELLER)
    .bind(Uuid::from_u128(0x500))
    .bind(listing_id)
    .bind(at)
    .execute(&mut **tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ($1, 1, $2, 'public_user', 'registered', NULL, 'resolving', NULL, 101, $3)",
    )
    .bind(listing_id)
    .bind(command_id)
    .bind(at)
    .execute(&mut **tx)
    .await
    .unwrap();
}

fn fence(
    work_id: Uuid,
    listing_id: Uuid,
    generation: u64,
    sequence: i64,
    token: Uuid,
) -> LookupFence {
    LookupFence::new(work_id, listing_id, generation, sequence, token).unwrap()
}

fn current_fence() -> LookupFence {
    fence(WORK, LISTING, 1, 7, FAKE_TOKEN)
}

fn timestamp(row: &Value, field: &str) -> Timestamp {
    row[field].as_str().unwrap().parse().unwrap()
}

async fn assert_deadline_crossed(
    tx: &mut Transaction<'_, Postgres>,
    deadline: Timestamp,
) -> Timestamp {
    let (started_at, crossed): (Timestamp, bool) = sqlx::query_as(
        "SELECT transaction_timestamp(), transaction_timestamp() < $1 AND $1 < clock_timestamp()",
    )
    .bind(deadline)
    .fetch_one(&mut **tx)
    .await
    .unwrap();
    assert!(crossed, "截止须严格晚于事务开始、早于数据库墙钟");
    started_at
}

fn assert_work_shape(row: &Value, state: &str) {
    assert_eq!(row["state"], state);
    for field in ["lease_owner", "token", "lease_until", "network_deadline"] {
        assert!(row[field].is_null(), "工作残留租约字段 {field}");
    }
    if state == "queued" {
        assert!(row["completed_at"].is_null());
        timestamp(row, "next_attempt_at");
    } else {
        assert!(row["next_attempt_at"].is_null());
        assert_eq!(timestamp(row, "completed_at"), timestamp(row, "updated_at"));
    }
}

fn assert_released(control: &Value) {
    for column in ["holder_work_id", "holder_fence", "holder_token", "permit_until"] {
        assert!(control[column].is_null(), "许可残留 {column}");
    }
}

#[tokio::test]
async fn postgres_enqueue_shape_and_duplicate() {
    let fixture = Fixture::new().await;
    let listing_id = Uuid::from_u128(0x202);
    let work_id = Uuid::from_u128(0x302);
    let mut tx = fixture.lock().await;
    seed_listing(&mut tx, listing_id, "1003", Uuid::from_u128(0x602)).await;
    DatabaseStore::test_lookup_enqueue(&mut tx, listing_id, work_id, registered_at())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let row = fixture.work(work_id).await;
    assert_work_shape(&row, "queued");
    assert_eq!(row["listing_id"], listing_id.to_string());
    assert_eq!(row["generation"], 1);
    assert_eq!(row["attempt_count"], 0);
    assert_eq!(row["fence"], 0);
    assert!(row["last_reason"].is_null());
    for field in ["next_attempt_at", "created_at", "updated_at"] {
        assert_eq!(timestamp(&row, field), registered_at());
    }
    let mut tx = fixture.lock().await;
    let error = DatabaseStore::test_lookup_enqueue(
        &mut tx,
        listing_id,
        Uuid::from_u128(0x303),
        registered_at(),
    )
    .await
    .expect_err("同一挂单不得再次入队第一代");
    assert_eq!(error.as_database_error().unwrap().code().as_deref(), Some("23505"));
    tx.rollback().await.unwrap();
    assert!(row == fixture.work(work_id).await);
}

#[tokio::test]
async fn postgres_cancel_queued_keeps_unrelated_and_terminal_rows() {
    let fixture = Fixture::new().await;
    fixture.grant(OTHER_WORK, OTHER_FAKE_TOKEN, 1, 120.0).await;
    let control = fixture.control().await;
    let other = fixture.work(OTHER_WORK).await;
    assert_eq!(fixture.cancel(LISTING).await, 1);
    let row = fixture.work(WORK).await;
    assert_work_shape(&row, "cancelled");
    assert_eq!(row["attempt_count"], 0);
    assert_eq!(fixture.cancel(LISTING).await, 0);
    assert_eq!(fixture.cancel(Uuid::from_u128(0x999)).await, 0);
    assert!(row == fixture.work(WORK).await);
    assert!(control == fixture.control().await);
    assert!(other == fixture.work(OTHER_WORK).await);
}

#[tokio::test]
async fn postgres_cancel_leased_guards_in_flight_until_original_deadline() {
    let fixture = Fixture::new().await;
    let (_, hard_deadline) = fixture.grant(WORK, FAKE_TOKEN, 1, 120.0).await;
    let before = fixture.control().await;
    assert_eq!(fixture.cancel(LISTING).await, 1);
    let (work, permit) = fixture.snapshot().await;
    assert_work_shape(&work, "cancelled");
    assert_eq!(work["attempt_count"], 1);
    assert_eq!(timestamp(&permit, "permit_until"), hard_deadline + TimeDelta::seconds(5));
    for field in ["holder_work_id", "holder_fence", "holder_token", "next_allowed_at"] {
        assert!(permit[field] == before[field], "撤销改动了在途栅栏字段 {field}");
    }
    assert_eq!(fixture.finish(current_fence(), None, None).await, LookupLeaseUpdate::Lost);
    assert!((work, permit) == fixture.snapshot().await, "撤销后旧提交改动了持久行");
}

#[tokio::test]
async fn postgres_cancel_expired_releases_only_matching_holder() {
    let fixture = Fixture::new().await;
    fixture.grant(WORK, FAKE_TOKEN, 2, -120.0).await;
    assert_eq!(fixture.cancel(LISTING).await, 1);
    assert_work_shape(&fixture.work(WORK).await, "cancelled");
    assert_released(&fixture.control().await);

    fixture.grant(OTHER_WORK, OTHER_FAKE_TOKEN, 2, -120.0).await;
    sqlx::query("UPDATE public.consignment_lookup_control SET holder_fence = 8 WHERE id = 1")
        .execute(fixture.pool())
        .await
        .unwrap();
    let before = fixture.control().await;
    assert_eq!(fixture.cancel(OTHER_LISTING).await, 1);
    assert_work_shape(&fixture.work(OTHER_WORK).await, "cancelled");
    assert!(before == fixture.control().await, "同工作但不同围栏的许可被撤销");
}

#[tokio::test]
async fn postgres_cancel_keeps_holder_with_same_work_and_fence_but_different_token() {
    let fixture = Fixture::new().await;
    fixture.grant(WORK, FAKE_TOKEN, 2, -120.0).await;
    sqlx::query("UPDATE public.consignment_lookup_control SET holder_token = $1 WHERE id = 1")
        .bind(OTHER_FAKE_TOKEN)
        .execute(fixture.pool())
        .await
        .unwrap();
    let before = fixture.control().await;
    assert_eq!(fixture.cancel(LISTING).await, 1);
    assert_work_shape(&fixture.work(WORK).await, "cancelled");
    assert!(before == fixture.control().await, "同工作、同围栏但不同 token 的许可被改动");
}

#[tokio::test]
async fn postgres_fence_identity_and_no_writes() {
    let fixture = Fixture::new().await;
    fixture.grant(WORK, FAKE_TOKEN, 2, 120.0).await;
    let before = fixture.snapshot().await;
    assert_eq!(fixture.check(current_fence(), None).await, Some(LookupLeaseUpdate::Applied(2)));
    for (label, wrong, expected) in [
        ("work", fence(OTHER_WORK, LISTING, 1, 7, FAKE_TOKEN), None),
        ("listing", fence(WORK, OTHER_LISTING, 1, 7, FAKE_TOKEN), None),
        ("generation", fence(WORK, LISTING, 2, 7, FAKE_TOKEN), None),
        ("fence", fence(WORK, LISTING, 1, 8, FAKE_TOKEN), Some(LookupLeaseUpdate::Lost)),
        ("token", fence(WORK, LISTING, 1, 7, OTHER_FAKE_TOKEN), Some(LookupLeaseUpdate::Lost)),
    ] {
        assert_eq!(fixture.check(wrong, None).await, expected, "{label}");
        let rate_limit = Some((LookupTransientReason::SourceRateLimited, None));
        assert_eq!(
            fixture.finish(wrong, None, rate_limit).await,
            LookupLeaseUpdate::Lost,
            "{label}"
        );
        assert!(before == fixture.snapshot().await, "{label} 改动了工作或全局许可");
    }
}

#[tokio::test]
async fn postgres_fence_expires_at_equality_and_wall_clock_cas() {
    let fixture = Fixture::new().await;
    let (lease_until, _) = fixture.grant(WORK, FAKE_TOKEN, 2, -120.0).await;
    // 让工作到期时全局许可仍活，避免许可边界掩盖工作边界。
    sqlx::query(
        "UPDATE public.consignment_lookup_control
         SET permit_until = statement_timestamp() + interval '5 minutes' WHERE id = 1",
    )
    .execute(fixture.pool())
    .await
    .unwrap();
    let before = fixture.snapshot().await;
    let before_expiry = lease_until - TimeDelta::microseconds(1);
    assert_eq!(
        fixture.check(current_fence(), Some(before_expiry)).await,
        Some(LookupLeaseUpdate::Applied(2))
    );
    for at in [Some(lease_until), Some(lease_until + TimeDelta::microseconds(1)), None] {
        assert_eq!(fixture.check(current_fence(), at).await, Some(LookupLeaseUpdate::Lost));
    }
    let failure = Some((LookupTransientReason::SourceRateLimited, None));
    assert_eq!(
        fixture.finish(current_fence(), Some(before_expiry), failure).await,
        LookupLeaseUpdate::Lost
    );
    assert!(before == fixture.snapshot().await, "过期提交改动了工作或全局许可");
}

#[tokio::test]
async fn postgres_fence_check_rejects_work_lease_expired_during_transaction() {
    let fixture = Fixture::new().await;
    fixture.grant(WORK, FAKE_TOKEN, 1, 120.0).await;
    let mut tx = fixture.lock().await;
    let lease_until: Timestamp = sqlx::query_scalar(
        "UPDATE public.consignment_lookup_work
         SET lease_until = transaction_timestamp() + interval '1 microsecond',
             network_deadline = transaction_timestamp() - interval '5 seconds'
         WHERE id = $1 RETURNING lease_until",
    )
    .bind(WORK)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_deadline_crossed(&mut tx, lease_until).await;
    assert_eq!(
        DatabaseStore::test_lookup_fence(&mut tx, current_fence(), None).await.unwrap(),
        Some(LookupLeaseUpdate::Lost)
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn postgres_finish_cas_rejects_permit_expired_during_transaction() {
    let fixture = Fixture::new().await;
    fixture.grant(WORK, FAKE_TOKEN, 1, 120.0).await;
    let mut tx = fixture.lock().await;
    let permit_until: Timestamp = sqlx::query_scalar(
        "UPDATE public.consignment_lookup_control
         SET permit_until = transaction_timestamp() + interval '1 microsecond'
         WHERE id = 1 RETURNING permit_until",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    // 以尚未到期的事务开始时刻核验初检，单独检验最终 CAS 采样墙钟。
    let started_at = assert_deadline_crossed(&mut tx, permit_until).await;
    assert_eq!(
        DatabaseStore::test_lookup_fence(&mut tx, current_fence(), Some(started_at)).await.unwrap(),
        Some(LookupLeaseUpdate::Applied(1))
    );
    assert_eq!(
        DatabaseStore::test_lookup_finish(&mut tx, current_fence(), Some(started_at), None)
            .await
            .unwrap(),
        LookupLeaseUpdate::Lost
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn postgres_fence_cannot_touch_newer_permit() {
    let fixture = Fixture::new().await;
    fixture.grant(WORK, FAKE_TOKEN, 1, 120.0).await;
    fixture.grant(OTHER_WORK, OTHER_FAKE_TOKEN, 1, 120.0).await;
    let before = fixture.snapshot().await;
    let failure = Some((LookupTransientReason::SourceRateLimited, None));
    assert_eq!(fixture.check(current_fence(), None).await, Some(LookupLeaseUpdate::Lost));
    assert_eq!(fixture.finish(current_fence(), None, failure).await, LookupLeaseUpdate::Lost);
    assert!(before == fixture.snapshot().await);
    assert_eq!(
        fixture.check(fence(OTHER_WORK, OTHER_LISTING, 1, 7, OTHER_FAKE_TOKEN), None).await,
        Some(LookupLeaseUpdate::Applied(1))
    );
}

#[tokio::test]
async fn postgres_retry_uses_five_delays_and_safe_reason() {
    let fixture = Fixture::new().await;
    let reason = LookupTransientReason::SourceTimeout;
    for (attempt, seconds) in [(1, 30), (2, 120), (3, 600), (4, 1800), (5, 7200)] {
        fixture.grant(WORK, FAKE_TOKEN, attempt, 120.0).await;
        let result = fixture.finish(current_fence(), None, Some((reason, None))).await;
        let (work, permit) = fixture.snapshot().await;
        assert_work_shape(&work, "queued");
        let expected = timestamp(&work, "updated_at") + TimeDelta::seconds(seconds);
        assert_eq!(result, LookupLeaseUpdate::Applied(Some(expected)), "第 {attempt} 次退避");
        assert_eq!(timestamp(&work, "next_attempt_at"), expected);
        assert_eq!(work["last_reason"], "source_timeout");
        assert_eq!(work["attempt_count"], attempt);
        assert_eq!(work["fence"], 7);
        assert_released(&permit);
        assert!(permit["cooldown_until"].is_null(), "非 429 不设置全局冷却");
    }
}

#[tokio::test]
async fn postgres_done_clears_all_holder_fields_without_shortening_start_interval() {
    let fixture = Fixture::new().await;
    fixture.grant(WORK, FAKE_TOKEN, 1, 120.0).await;
    let start = fixture.control().await["next_allowed_at"].clone();
    assert_eq!(fixture.finish(current_fence(), None, None).await, LookupLeaseUpdate::Applied(None));
    let (work, permit) = fixture.snapshot().await;
    assert_work_shape(&work, "done");
    assert_eq!(work["attempt_count"], 1);
    assert!(work["last_reason"].is_null());
    assert_released(&permit);
    assert_eq!(permit["next_allowed_at"], start);
    assert!(permit["cooldown_until"].is_null());
    assert_eq!(fixture.cancel(LISTING).await, 0);
    assert_eq!(fixture.finish(current_fence(), None, None).await, LookupLeaseUpdate::Lost);
    assert!((work, permit) == fixture.snapshot().await);
}

#[tokio::test]
async fn postgres_exhausted_clears_holder_and_never_requeues() {
    let fixture = Fixture::new().await;
    fixture.grant(WORK, FAKE_TOKEN, 6, 120.0).await;
    let failure = Some((LookupTransientReason::SourceUnavailable, None));
    assert_eq!(
        fixture.finish(current_fence(), None, failure).await,
        LookupLeaseUpdate::Applied(None)
    );
    let (work, permit) = fixture.snapshot().await;
    assert_work_shape(&work, "done");
    assert_eq!(work["attempt_count"], 6);
    assert_eq!(work["last_reason"], "source_unavailable");
    assert_released(&permit);
    assert!(permit["cooldown_until"].is_null());
}

async fn finish_rate_limit(
    fixture: &Fixture,
    attempt: i16,
    retry_hint: Option<Duration>,
) -> (Value, Value) {
    fixture.grant(WORK, FAKE_TOKEN, attempt, 120.0).await;
    let failure =
        Some((LookupTransientReason::SourceRateLimited, retry_hint.map(LookupRetryAfter::new)));
    let result = fixture.finish(current_fence(), None, failure).await;
    let (work, permit) = fixture.snapshot().await;
    if attempt == 6 {
        assert_work_shape(&work, "done");
        assert_eq!(result, LookupLeaseUpdate::Applied(None));
    } else {
        assert_work_shape(&work, "queued");
        assert_eq!(result, LookupLeaseUpdate::Applied(Some(timestamp(&work, "next_attempt_at"))));
    }
    assert_eq!(work["attempt_count"], attempt);
    assert_eq!(work["last_reason"], "source_rate_limited");
    assert_released(&permit);
    (work, permit)
}

#[tokio::test]
async fn postgres_cooldown_uses_backoff_over_short_or_missing_hint() {
    let fixture = Fixture::new().await;
    for hint in [Some(Duration::from_secs(1)), None] {
        let (work, permit) = finish_rate_limit(&fixture, 1, hint).await;
        let expected = timestamp(&work, "updated_at") + TimeDelta::seconds(30);
        assert_eq!(timestamp(&permit, "cooldown_until"), expected);
        assert_eq!(timestamp(&work, "next_attempt_at"), expected);
    }
}

#[tokio::test]
async fn postgres_cooldown_accepts_hint_and_caps_at_six_hours() {
    let fixture = Fixture::new().await;
    for (hint, seconds) in [(Duration::from_secs(45), 45), (Duration::from_secs(86_400), 21_600)] {
        let (work, permit) = finish_rate_limit(&fixture, 1, Some(hint)).await;
        let at = timestamp(&work, "updated_at");
        assert_eq!(timestamp(&permit, "cooldown_until"), at + TimeDelta::seconds(seconds));
        assert_eq!(timestamp(&work, "next_attempt_at"), at + TimeDelta::seconds(30));
    }
}

#[tokio::test]
async fn postgres_cooldown_preserves_later_persisted_deadline() {
    let fixture = Fixture::new().await;
    let later: Timestamp = sqlx::query_scalar(
        "UPDATE public.consignment_lookup_control
         SET cooldown_until = statement_timestamp() + interval '7 hours' WHERE id = 1
         RETURNING cooldown_until",
    )
    .fetch_one(fixture.pool())
    .await
    .unwrap();
    let (_, permit) = finish_rate_limit(&fixture, 1, Some(Duration::from_secs(86_400))).await;
    assert_eq!(timestamp(&permit, "cooldown_until"), later);
}

#[tokio::test]
async fn postgres_exhausted_rate_limit_still_sets_cooldown() {
    let fixture = Fixture::new().await;
    for (hint, hours) in
        [(None, 2), (Some(Duration::from_hours(1)), 2), (Some(Duration::from_hours(24)), 6)]
    {
        let (work, permit) = finish_rate_limit(&fixture, 6, hint).await;
        assert_eq!(
            timestamp(&permit, "cooldown_until"),
            timestamp(&work, "updated_at") + TimeDelta::hours(hours)
        );
    }
}

#[tokio::test]
async fn postgres_admission_rejects_heartbeat_expired_during_transaction() {
    let fixture = Fixture::new().await;
    let mut tx = fixture.lock().await;
    let worker_ready_until: Timestamp = sqlx::query_scalar(
        "UPDATE public.consignment_lookup_control
         SET admitting = true, worker_ready_until = transaction_timestamp() + interval '1 microsecond'
         WHERE id = 1 RETURNING worker_ready_until",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_deadline_crossed(&mut tx, worker_ready_until).await;
    let (permit, ready) = DatabaseStore::test_lookup_admission(&mut tx).await.unwrap();
    assert!(permit.admitting);
    assert!(!ready, "事务开始时尚有效的心跳已经过期");
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn postgres_admission_is_unlocked_and_requires_fresh_enabled_heartbeat() {
    let fixture = Fixture::new().await;
    let mut holder = fixture.lock().await;
    let mut reader = fixture.pool().begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout TO '200ms'").execute(&mut *reader).await.unwrap();
    let (permit, ready) = DatabaseStore::test_lookup_admission(&mut reader).await.unwrap();
    assert_eq!(permit.service_epoch, 1);
    assert!(!permit.admitting);
    assert!(!ready);
    reader.commit().await.unwrap();

    for (admitting, interval, ready) in [
        (true, None, false),
        (true, Some(-60.0), false),
        (true, Some(60.0), true),
        (false, Some(60.0), false),
    ] {
        sqlx::query(
            "UPDATE public.consignment_lookup_control
             SET admitting = $1, enabled_epoch = 3,
                 worker_ready_until = statement_timestamp() + make_interval(secs => $2)
             WHERE id = 1",
        )
        .bind(admitting)
        .bind(interval)
        .execute(&mut *holder)
        .await
        .unwrap();
        let (permit, is_ready) = DatabaseStore::test_lookup_admission(&mut holder).await.unwrap();
        assert_eq!(permit.service_epoch, 3);
        assert_eq!(permit.admitting, admitting);
        assert_eq!(is_ready, ready);
    }
    holder.commit().await.unwrap();
}

#[tokio::test]
async fn postgres_control_lock_orders_two_transactions_before_work() {
    let fixture = Fixture::new().await;
    fixture.grant(WORK, FAKE_TOKEN, 1, 120.0).await;
    let mut owner = fixture.lock().await;
    let owner_pid: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *owner).await.unwrap();

    let mut contender = fixture.pool().begin().await.unwrap();
    let contender_pid: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *contender).await.unwrap();
    sqlx::query("SET LOCAL lock_timeout TO '10s'").execute(&mut *contender).await.unwrap();
    let contender_task = tokio::spawn(async move {
        DatabaseStore::test_lookup_lock(&mut contender).await.unwrap();
        let cancelled = DatabaseStore::test_lookup_cancel(&mut contender, LISTING).await.unwrap();
        contender.commit().await.unwrap();
        cancelled
    });
    let mut observer = fixture.pool().acquire().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT $1 = ANY(pg_blocking_pids($2))")
                .bind(owner_pid)
                .bind(contender_pid)
                .fetch_one(&mut *observer)
                .await
                .unwrap();
            if blocked {
                break;
            }
            assert!(!contender_task.is_finished(), "未等单例锁就取得了工作锁或写入了结果");
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("数据库没有观测到预期的单例锁等待");

    let result =
        DatabaseStore::test_lookup_finish(&mut owner, current_fence(), None, None).await.unwrap();
    assert_eq!(result, LookupLeaseUpdate::Applied(None));
    owner.commit().await.unwrap();
    assert_eq!(contender_task.await.unwrap(), 0, "后到撤销不得覆盖已完成工作");
    let (work, permit) = fixture.snapshot().await;
    assert_work_shape(&work, "done");
    assert_released(&permit);
}

#[tokio::test]
async fn postgres_fence_checks_work_and_permit_credentials_independently() {
    let fixture = Fixture::new().await;
    for change in [
        "UPDATE public.consignment_lookup_work SET fence = 8 WHERE id = $1",
        "UPDATE public.consignment_lookup_work SET token = $2 WHERE id = $1",
        "UPDATE public.consignment_lookup_control SET holder_fence = 8 WHERE holder_work_id = $1",
        "UPDATE public.consignment_lookup_control SET holder_token = $2 WHERE holder_work_id = $1",
    ] {
        fixture.grant(WORK, FAKE_TOKEN, 1, 120.0).await;
        let query = sqlx::query(change).bind(WORK);
        if change.contains("$2") {
            query.bind(OTHER_FAKE_TOKEN).execute(fixture.pool()).await.unwrap();
        } else {
            query.execute(fixture.pool()).await.unwrap();
        }
        let before = fixture.snapshot().await;
        assert_eq!(
            fixture.check(current_fence(), None).await,
            Some(LookupLeaseUpdate::Lost),
            "{change}"
        );
        assert_eq!(fixture.finish(current_fence(), None, None).await, LookupLeaseUpdate::Lost);
        assert!(before == fixture.snapshot().await);
    }
}

#[tokio::test]
async fn postgres_permit_deadline_is_independent_of_work_deadline() {
    let fixture = Fixture::new().await;
    fixture.grant(WORK, FAKE_TOKEN, 1, 120.0).await;
    let permit_until: Timestamp = sqlx::query_scalar(
        "UPDATE public.consignment_lookup_control
         SET permit_until = statement_timestamp() - interval '1 minute' WHERE id = 1
         RETURNING permit_until",
    )
    .fetch_one(fixture.pool())
    .await
    .unwrap();
    let before = fixture.snapshot().await;
    let before_expiry = permit_until - TimeDelta::microseconds(1);
    assert_eq!(
        fixture.check(current_fence(), Some(before_expiry)).await,
        Some(LookupLeaseUpdate::Applied(1))
    );
    for at in [Some(permit_until), None] {
        assert_eq!(fixture.check(current_fence(), at).await, Some(LookupLeaseUpdate::Lost));
    }
    assert_eq!(
        fixture.finish(current_fence(), Some(before_expiry), None).await,
        LookupLeaseUpdate::Lost
    );
    assert!(before == fixture.snapshot().await);
}
