//! 崩溃回收、原围栏 CAS、时间栅栏和原子终态。

use std::time::Duration as WallDuration;

use auction_application::consignment::{
    AutoCloseCandidate, AutoCloseEvidence, ConsignmentError, ListingLookupClaim,
    ListingMaintenance, LookupAbandonment, LookupDispatch, LookupFence, LookupLeaseUpdate,
    LookupWorkStore,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_time::{Duration, Timestamp};
use tokio::sync::Barrier;
use uuid::Uuid;

use super::fixture::{
    Fixture, LISTING, OTHER_WORK, WORK, assert_requeued_after_failure, deadline_command,
    exhausted_command,
};
use crate::support::fixtures;

#[tokio::test]
async fn postgres_consignment_work_reap_backoff_and_sixth_attempt() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let work = fixture.work();
    for (attempt, delay) in [(1_u8, 30), (2, 120), (3, 600), (4, 1800), (5, 7200)] {
        fixture.due_now().await;
        let claim = fixture.claim(1).await.unwrap();
        assert_eq!(claim.fence().work_id(), WORK);
        assert_eq!(claim.attempt(), attempt);
        assert_eq!(claim.fence().fence(), i64::from(attempt));
        assert_eq!(fixture.attempts().await, i64::from(attempt));
        assert_eq!(work.reap_expired().await.unwrap(), 0, "未过期不可回收");
        fixture.expire_lease(claim.fence()).await;
        // 丢弃原领取并新装配一个 Worker 端口，只从数据库恢复。
        drop(claim);
        assert_eq!(fixture.work().reap_expired().await.unwrap(), 1);
        assert_eq!(work.reap_expired().await.unwrap(), 0);
        let (state, attempts, next_at, updated_at, reason, clear): (String, i16, Timestamp, Timestamp, String, bool) = sqlx::query_as(
            "SELECT state, attempt_count, next_attempt_at, updated_at, last_reason,
                 token IS NULL AND lease_owner IS NULL AND lease_until IS NULL AND network_deadline IS NULL
             FROM public.consignment_lookup_work WHERE id = $1",
        ).bind(WORK).fetch_one(fixture.pool()).await.unwrap();
        assert_eq!(
            (state.as_str(), attempts, reason.as_str()),
            ("queued", i16::from(attempt), "source_timeout")
        );
        assert!(clear);
        assert_eq!(next_at - updated_at, Duration::seconds(delay));
    }
    fixture.due_now().await;
    let last = fixture.claim(1).await.unwrap();
    assert_eq!(last.attempt(), 6);
    fixture.expire_lease(last.fence()).await;
    assert_eq!(work.reap_expired().await.unwrap(), 1);
    assert_eq!(work.reap_expired().await.unwrap(), 0);
    assert_terminal(
        &fixture,
        "rejected",
        "lookup_exhausted",
        "worker",
        "apply_lookup",
        exhausted_command(last.fence()),
        6,
    )
    .await;
    fixture.allow_now().await;
    assert_eq!(fixture.claim(1).await.unwrap().fence().work_id(), OTHER_WORK);
}

#[tokio::test]
async fn postgres_consignment_work_reap_total_deadline() {
    let fixture = Fixture::deadline_seconds(-60.0).await;
    fixture.db.sync_admission(true).await.unwrap();
    assert!(fixture.claim(1).await.is_none());
    fixture.seed_lease(6, true).await;
    let work = fixture.work();
    assert_eq!(work.reap_expired().await.unwrap(), 1);
    assert_eq!(work.reap_expired().await.unwrap(), 0);
    let cmd = deadline_command(AutoCloseCandidate {
        listing_id: LISTING,
        evidence: AutoCloseEvidence::LookupDeadline(fixture.expires().await),
    });
    assert_terminal(
        &fixture,
        "expired",
        "lookup_deadline",
        "system",
        "auto_close",
        (cmd.command_id, cmd.request_hash),
        6,
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_work_reap_advances_past_locked_work_rows() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    fixture.expire_other_lease().await;
    fixture.seed_exhausted_backlog(16).await;
    let mut lock = fixture.owner.postgres_pool().begin().await.unwrap();
    let locked: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM public.consignment_lookup_work WHERE id <> $1 FOR UPDATE",
    )
    .bind(OTHER_WORK)
    .fetch_all(&mut *lock)
    .await
    .unwrap();
    assert_eq!(locked.len(), 16);

    let work = fixture.work();
    assert_healthy_lease_recovered(&fixture, &work).await;
    lock.rollback().await.unwrap();
    assert_backlog_recovered(&fixture, &work).await;
}

#[tokio::test]
async fn postgres_consignment_work_reap_advances_past_rejected_work_updates() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    fixture.expire_other_lease().await;
    fixture.seed_exhausted_backlog(16).await;
    sqlx::raw_sql(
        "CREATE FUNCTION public.fake_reaper_work_reject() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION '测试工作更新失败' USING ERRCODE = '23514'; END;
         $$;
         CREATE TRIGGER fake_reaper_work_reject BEFORE UPDATE ON public.consignment_lookup_work
             FOR EACH ROW WHEN (OLD.attempt_count = 6)
             EXECUTE FUNCTION public.fake_reaper_work_reject()",
    )
    .execute(fixture.owner.postgres_pool())
    .await
    .unwrap();

    let work = fixture.work();
    assert_healthy_lease_recovered(&fixture, &work).await;
    sqlx::query("DROP TRIGGER fake_reaper_work_reject ON public.consignment_lookup_work")
        .execute(fixture.owner.postgres_pool())
        .await
        .unwrap();
    assert_backlog_recovered(&fixture, &work).await;
}

async fn assert_healthy_lease_recovered(fixture: &Fixture, work: &impl LookupWorkStore) {
    let mut recovered = work.reap_expired().await.unwrap();
    if recovered == 0 {
        recovered = work.reap_expired().await.unwrap();
    }
    assert_eq!(recovered, 1, "最早的 16 条无法回收时，健康租约须在至多两轮内回收");
    assert_eq!(reap_work_states(fixture).await, (16, 1, 0));
    assert_requeued_after_failure(fixture).await;
}

async fn assert_backlog_recovered(fixture: &Fixture, work: &impl LookupWorkStore) {
    assert_eq!(work.reap_expired().await, Ok(16), "故障解除后可回收之前跳过的 16 条");
    assert_eq!(reap_work_states(fixture).await, (0, 1, 16));
}

async fn reap_work_states(fixture: &Fixture) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT count(*) FILTER (WHERE state = 'leased'), count(*) FILTER (WHERE state = 'queued'),
                count(*) FILTER (WHERE state = 'done') FROM public.consignment_lookup_work",
    )
    .fetch_one(fixture.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn postgres_consignment_work_reap_candidate_query_times_out_under_table_lock() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    fixture.seed_lease(1, true).await;
    let timeout: String =
        sqlx::query_scalar("SHOW statement_timeout").fetch_one(fixture.pool()).await.unwrap();
    assert_eq!(timeout, "0");
    let before = fixture.work_snapshot().await;
    let mut lock = fixture.owner.postgres_pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE public.consignment_lookup_work IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let backend: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *lock).await.unwrap();
    let work = fixture.work();
    let (outcome, blocked) = tokio::join!(
        tokio::time::timeout(WallDuration::from_secs(6), work.reap_expired()),
        fixtures::wait_for_condition(WallDuration::from_millis(800), || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))",
            ).bind(backend).fetch_one(fixture.owner.postgres_pool()).await.unwrap()
        }),
    );
    lock.rollback().await.unwrap();
    assert!(blocked, "独立连接应确认候选查询在等表锁");
    assert_eq!(
        outcome.expect("仍持有表锁时，回收必须在预算内返回"),
        Err(ConsignmentError::Unavailable)
    );
    assert!(before == fixture.work_snapshot().await);
    assert_eq!(work.reap_expired().await, Ok(1));
}

#[tokio::test]
async fn postgres_consignment_work_stale_fences_are_read_only() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let work = fixture.work();
    let older = fixture.claim(1).await.unwrap();
    fixture.expire_lease(older.fence()).await;
    assert_eq!(work.reap_expired().await.unwrap(), 1);
    fixture.due_now().await;
    let current = fixture.claim(1).await.unwrap();
    assert_eq!(current.fence().work_id(), older.fence().work_id());
    assert_eq!(current.fence().fence(), older.fence().fence() + 1);
    let active = current.fence();
    let before = fixture.work_snapshot().await;
    for (listing_id, generation, fence, token) in [
        (LISTING, 1, older.fence().fence(), active.token()),
        (LISTING, 1, active.fence(), older.fence().token()),
        (LISTING, 2, active.fence(), active.token()),
        (Uuid::from_u128(0x2f10), 1, active.fence(), active.token()),
    ] {
        let stale = LookupFence::new(WORK, listing_id, generation, fence, token).unwrap();
        let stale_claim = ListingLookupClaim::new(
            stale,
            current.identity().clone(),
            1,
            current.attempt(),
            current.deadline(),
            current.lease_until(),
            current.credential().cloned(),
        )
        .unwrap();
        assert_eq!(work.renew(stale).await.unwrap(), LookupLeaseUpdate::Lost);
        assert_eq!(work.confirm_dispatch(&stale_claim, false).await.unwrap(), LookupDispatch::Lost);
        for status in [
            LookupAbandonment::NotSent,
            LookupAbandonment::RequestFinished,
            LookupAbandonment::MayBeInFlight,
        ] {
            assert_eq!(work.abandon(stale, status).await.unwrap(), LookupLeaseUpdate::Lost);
        }
        assert!(before == fixture.work_snapshot().await, "失效围栏不能写工作或许可");
    }
    assert_eq!(
        work.confirm_dispatch(&current, false).await.unwrap(),
        LookupDispatch::Allowed { deadline: current.deadline() }
    );
}

#[tokio::test]
async fn postgres_consignment_work_renew_rejects_another_permit_token() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let fence = fixture.seed_lease(1, false).await;
    replace_permit_token(&fixture).await;
    let before = fixture.work_snapshot().await;
    assert_eq!(fixture.work().renew(fence).await.unwrap(), LookupLeaseUpdate::Lost);
    assert!(before == fixture.work_snapshot().await);
}

#[tokio::test]
async fn postgres_consignment_work_abandon_rejects_another_permit_token() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let fence = fixture.seed_lease(1, false).await;
    replace_permit_token(&fixture).await;
    let before = fixture.work_snapshot().await;
    assert_eq!(
        fixture.work().abandon(fence, LookupAbandonment::NotSent).await.unwrap(),
        LookupLeaseUpdate::Lost
    );
    assert!(before == fixture.work_snapshot().await);
}

#[tokio::test]
async fn postgres_consignment_work_dispatch_rejects_another_permit_token() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let claim = fixture.claim(1).await.unwrap();
    let work = fixture.work();
    assert_eq!(
        work.confirm_dispatch(&claim, false).await.unwrap(),
        LookupDispatch::Allowed { deadline: claim.deadline() }
    );

    replace_permit_token(&fixture).await;
    assert_dispatch_lost_without_writes(&fixture, &claim).await;
}

#[tokio::test]
async fn postgres_consignment_work_dispatch_rejects_lot_created_after_claim() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let claim = fixture.claim(1).await.unwrap();
    assert_eq!(claim.fence().listing_id(), LISTING);
    assert_eq!(
        fixture.work().confirm_dispatch(&claim, false).await.unwrap(),
        LookupDispatch::Allowed { deadline: claim.deadline() }
    );

    let mut tx = fixture.owner.postgres_pool().begin().await.unwrap();
    let deadline: Timestamp = sqlx::query_scalar("SELECT clock_timestamp() + interval '6 hours'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "WITH auction AS (
             INSERT INTO public.auctions
                 (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
                  scheduled_end_at, effective_end_at, policy_snapshot)
             VALUES ($1, 'main', 1, 'Fake Main', 'live', $2::timestamptz - interval '2 hours',
                     $2 - interval '3 hours', $2, $2, $3) RETURNING id
         ), equipment AS (
             INSERT INTO public.equipment
                 (id, world_id, external_equipment_id, name, category, collected_at)
             VALUES ($4, 'main', '000941', 'Fake Dispatch Equipment', 'Weapon', clock_timestamp())
             RETURNING id
         )
         INSERT INTO public.lots
             (id, auction_id, equipment_id, lot_number, status, start_price, category_key, category_number)
         SELECT $5, auction.id, equipment.id, 1, 'unsold', 50000, 'Weapon', 1 FROM auction, equipment",
    )
    .bind(Uuid::new_v4()).bind(deadline).bind(fixtures::policy_snapshot(deadline, 50_000, 30_000))
    .bind(Uuid::new_v4()).bind(Uuid::new_v4()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    assert_dispatch_lost_without_writes(&fixture, &claim).await;
}

async fn assert_dispatch_lost_without_writes(fixture: &Fixture, claim: &ListingLookupClaim) {
    let before = fixture.work_snapshot().await;
    let registration_before = dispatch_registration_snapshot(fixture).await;
    assert_eq!(fixture.work().confirm_dispatch(claim, false).await.unwrap(), LookupDispatch::Lost);
    assert!(before == fixture.work_snapshot().await);
    assert!(registration_before == dispatch_registration_snapshot(fixture).await);
}

async fn dispatch_registration_snapshot(fixture: &Fixture) -> String {
    sqlx::query_scalar(
        "SELECT md5(concat(
             (SELECT jsonb_agg(to_jsonb(l) || jsonb_build_object('xmin', xmin::text) ORDER BY id)
              FROM public.consignment_listings l),
             (SELECT jsonb_agg(xmin::text ORDER BY listing_id) FROM public.consignment_registration_secrets),
             (SELECT count(*) FROM public.consignment_events),
             (SELECT count(*) FROM public.consignment_commands),
             (SELECT count(*) FROM public.consignment_listing_auction_conflicts),
             (SELECT count(*) FROM public.audit_records)))",
    )
    .fetch_one(fixture.owner.postgres_pool()).await.unwrap()
}

async fn replace_permit_token(fixture: &Fixture) {
    sqlx::query("UPDATE public.consignment_lookup_control SET holder_token = $1 WHERE id = 1")
        .bind(Uuid::new_v4())
        .execute(fixture.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn postgres_consignment_work_renew_preserves_network_deadline() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let claim = fixture.claim(1).await.unwrap();
    let work = fixture.work();
    sqlx::query(
        "WITH changed AS (
             UPDATE public.consignment_lookup_work SET lease_until = network_deadline + interval '1 second'
             WHERE id = $1 RETURNING lease_until
         )
         UPDATE public.consignment_lookup_control c SET permit_until = changed.lease_until FROM changed WHERE c.id = 1",
    ).bind(WORK).execute(fixture.pool()).await.unwrap();
    let limit = claim.deadline() + Duration::seconds(5);
    for _ in 0..2 {
        assert_eq!(work.renew(claim.fence()).await.unwrap(), LookupLeaseUpdate::Applied(limit));
        let persisted: (Timestamp, Timestamp) = sqlx::query_as(
            "SELECT network_deadline, lease_until FROM public.consignment_lookup_work WHERE id = $1",
        ).bind(WORK).fetch_one(fixture.pool()).await.unwrap();
        assert_eq!(persisted, (claim.deadline(), limit));
        assert_eq!(work.permit_status().await.unwrap().permit_until, Some(limit));
    }
    sqlx::query(
        "UPDATE public.consignment_lookup_work SET network_deadline = statement_timestamp() - interval '1 millisecond'
         WHERE id = $1",
    ).bind(WORK).execute(fixture.pool()).await.unwrap();
    assert_eq!(work.confirm_dispatch(&claim, false).await.unwrap(), LookupDispatch::Lost);
    fixture.expire_lease(claim.fence()).await;
    let before = fixture.work_snapshot().await;
    assert_eq!(work.renew(claim.fence()).await.unwrap(), LookupLeaseUpdate::Lost);
    assert_eq!(
        work.abandon(claim.fence(), LookupAbandonment::NotSent).await.unwrap(),
        LookupLeaseUpdate::Lost
    );
    assert!(before == fixture.work_snapshot().await);
}

#[tokio::test]
async fn postgres_consignment_work_abandon_time_fence() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let work = fixture.work();
    let first = fixture.claim(1).await.unwrap();
    assert_eq!(
        work.abandon(first.fence(), LookupAbandonment::MayBeInFlight).await.unwrap(),
        LookupLeaseUpdate::Applied(())
    );
    assert_eq!(work.confirm_dispatch(&first, false).await.unwrap(), LookupDispatch::Lost);
    fixture.allow_now().await;
    assert_eq!(
        work.permit_status().await.unwrap().permit_until,
        Some(first.deadline() + Duration::seconds(5))
    );
    assert!(fixture.claim(1).await.is_none());
    let snapshot = fixture.work_snapshot().await;
    assert_eq!(
        work.abandon(first.fence(), LookupAbandonment::NotSent).await.unwrap(),
        LookupLeaseUpdate::Lost
    );
    assert!(snapshot == fixture.work_snapshot().await);
    sqlx::query("UPDATE public.consignment_lookup_control SET permit_until = statement_timestamp() WHERE id = 1")
        .execute(fixture.pool()).await.unwrap();
    let second = fixture.claim(1).await.unwrap();
    assert_eq!(second.fence().work_id(), OTHER_WORK);
    let next_allowed_at = work.permit_status().await.unwrap().next_allowed_at;
    assert_eq!(
        work.abandon(second.fence(), LookupAbandonment::NotSent).await.unwrap(),
        LookupLeaseUpdate::Applied(())
    );
    let permit = work.permit_status().await.unwrap();
    assert_eq!(permit.permit_until, None);
    assert_eq!(permit.next_allowed_at, next_allowed_at, "未发网退场不缩短原起始间隔");
    assert_eq!(fixture.attempts().await, 2);
    let before = fixture.work_snapshot().await;
    assert!(fixture.claim(1).await.is_none(), "间隔内没有新的许可");
    assert!(before == fixture.work_snapshot().await);
    fixture.allow_now().await;
    let next = fixture.claim(1).await.unwrap();
    assert_eq!(next.fence().work_id(), OTHER_WORK);
    assert_eq!(next.attempt(), 2, "再次领取从上次已持久化的计次继续");
}

#[tokio::test]
async fn postgres_consignment_work_reap_keeps_a_newer_permit() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let old = fixture.claim(1).await.unwrap();
    fixture.expire_lease(old.fence()).await;
    fixture.allow_now().await;
    let newer = fixture.claim(1).await.unwrap();
    assert_eq!(newer.fence().work_id(), OTHER_WORK);
    let work = fixture.work();
    let permit_revision = permit_row_revision(&fixture).await;
    assert_eq!(work.reap_expired().await.unwrap(), 1);
    assert_eq!(permit_row_revision(&fixture).await, permit_revision);
    assert_eq!(work.permit_status().await.unwrap().permit_until, Some(newer.lease_until()));
    assert_eq!(
        work.confirm_dispatch(&newer, false).await.unwrap(),
        LookupDispatch::Allowed { deadline: newer.deadline() }
    );
}

async fn permit_row_revision(fixture: &Fixture) -> String {
    sqlx::query_scalar("SELECT xmin::text FROM public.consignment_lookup_control WHERE id = 1")
        .fetch_one(fixture.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn postgres_consignment_work_reaper_refuses_replaced_candidate() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    fixture.seed_lease(1, true).await;
    let mut replacement = fixture.owner.postgres_pool().begin().await.unwrap();
    sqlx::query("SELECT id FROM public.consignment_lookup_control WHERE id = 1 FOR UPDATE")
        .fetch_one(&mut *replacement)
        .await
        .unwrap();
    let backend: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *replacement).await.unwrap();
    let work = fixture.work();
    let reap = tokio::spawn(async move { work.reap_expired().await });
    tokio::time::timeout(WallDuration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))
                )",
            )
            .bind(backend)
            .fetch_one(fixture.owner.postgres_pool())
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("回收已预读候选并等候许可行锁");
    sqlx::query(
        "WITH changed AS (
             UPDATE public.consignment_lookup_work SET token = $2, fence = fence + 1
             WHERE id = $1 RETURNING fence
         )
         UPDATE public.consignment_lookup_control c
         SET holder_token = $2, holder_fence = changed.fence FROM changed WHERE c.id = 1",
    )
    .bind(WORK)
    .bind(Uuid::new_v4())
    .execute(&mut *replacement)
    .await
    .unwrap();
    replacement.commit().await.unwrap();
    assert_eq!(reap.await.unwrap().unwrap(), 0);
    let state: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_lookup_work WHERE id = $1")
            .bind(WORK)
            .fetch_one(fixture.pool())
            .await
            .unwrap();
    assert_eq!(state, "leased");
    assert_eq!(fixture.work().reap_expired().await.unwrap(), 1);
}

#[tokio::test]
async fn postgres_consignment_work_two_reapers_commit_one_terminal() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let fence = fixture.seed_lease(6, true).await;
    let a = fixture.work();
    let b = fixture.work();
    let ready = Barrier::new(2);
    let (first, second) = tokio::join!(
        async {
            ready.wait().await;
            a.reap_expired().await.unwrap()
        },
        async {
            ready.wait().await;
            b.reap_expired().await.unwrap()
        },
    );
    assert_eq!(first + second, 1);
    assert_terminal(
        &fixture,
        "rejected",
        "lookup_exhausted",
        "worker",
        "apply_lookup",
        exhausted_command(fence),
        6,
    )
    .await;
}

#[derive(sqlx::FromRow)]
struct CommittedTerminal {
    state: String,
    reason_code: String,
    state_version: i64,
    work_state: String,
    attempt_count: i16,
    work_finished: bool,
    secret_retired: bool,
    matching_facts: bool,
    actor_kind: String,
    operation: String,
    request_digest: Vec<u8>,
}

async fn assert_terminal(
    fixture: &Fixture,
    state: &str,
    reason: &str,
    actor: &str,
    operation: &str,
    identity: (CommandId, RequestHash),
    attempt: i16,
) {
    let result = sqlx::query_as::<_, CommittedTerminal>(
        "SELECT l.state, l.reason_code, l.state_version, w.state AS work_state, w.attempt_count,
                w.lease_owner IS NULL AND w.token IS NULL AND w.lease_until IS NULL
                    AND w.network_deadline IS NULL AND w.completed_at IS NOT NULL AS work_finished,
                s.retired_at = l.updated_at AND s.ciphertext IS NULL AND s.nonce IS NULL
                    AND s.encryption_key_id IS NULL AND s.fingerprint IS NULL AS secret_retired,
                audit.request_id IS NULL AND audit.occurred_at = e.occurred_at
                    AND audit.actor_type = e.actor_kind
                    AND audit.action = 'consignment.' || c.operation AND audit.result = 'success'
                    AND c.result_body->>'reason_code' = l.reason_code
                    AND c.result_body->>'disposition' = e.event_kind AND e.to_state = l.state AS matching_facts,
                e.actor_kind, c.operation, c.request_digest
         FROM public.consignment_listings l
         JOIN public.consignment_lookup_work w ON w.listing_id = l.id
         JOIN public.consignment_registration_secrets s ON s.listing_id = l.id
         JOIN public.consignment_events e ON e.listing_id = l.id AND e.state_version = l.state_version
         JOIN public.consignment_commands c ON c.command_id = e.command_id
         JOIN public.audit_records audit ON audit.metadata->>'command_id' = c.command_id::text
         WHERE l.id = $1 AND c.command_id = $2",
    ).bind(LISTING).bind(identity.0.as_uuid()).fetch_one(fixture.pool()).await.unwrap();
    assert_eq!(result.state, state);
    assert_eq!(result.reason_code, reason);
    assert_eq!(result.state_version, 2);
    assert_eq!(result.work_state, "done");
    assert_eq!(result.attempt_count, attempt);
    assert!(result.work_finished && result.secret_retired && result.matching_facts);
    assert_eq!(result.actor_kind, actor);
    assert_eq!(result.operation, operation);
    assert_eq!(result.request_digest, identity.1.as_bytes());
}
