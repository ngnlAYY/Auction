//! 真实运行账号下领取、暂停与单例全局节流。

use std::time::Duration;

use auction_application::consignment::{
    ListingMaintenance, LookupAbandonment, LookupDispatch, LookupLeaseUpdate, LookupWorkStore,
};
use auction_application::runtime_switches::{
    RuntimeSwitchCommand, RuntimeSwitchName, RuntimeSwitchOperation, RuntimeSwitchStore,
};
use auction_domain::equipment::EquipmentKeyEnvelope;
use auction_time::{Duration as TimeDelta, Timestamp};
use serde_json::json;
use tokio::sync::Barrier;
use uuid::Uuid;

use super::fixture::{Fixture, LISTING, OTHER_WORK, REGISTERED_AAD, SELLER, WORK, request};

#[tokio::test]
async fn postgres_consignment_work_concurrent_claim() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let barrier = Barrier::new(2);
    let worker_a = fixture.work();
    let worker_b = fixture.work();
    let (a, b) = tokio::join!(
        async {
            barrier.wait().await;
            worker_a.claim(request(1, 1000)).await.unwrap()
        },
        async {
            barrier.wait().await;
            worker_b.claim(request(1, 1000)).await.unwrap()
        },
    );
    assert_ne!(a.is_some(), b.is_some());
    let chosen = a.or(b).unwrap();
    assert_eq!(chosen.attempt(), 1);
    assert_eq!(chosen.fence().fence(), 1);
    assert!(chosen.credential().is_ok());
    assert_eq!(fixture.attempts().await, 1);
    let permit = fixture.work().permit_status().await.unwrap();
    assert_eq!(permit.permit_until, Some(chosen.lease_until()));
    assert!(permit.admitting);
    assert_eq!(permit.service_epoch, 1);
    let (at, same_holder, owner): (Timestamp, bool, String) = sqlx::query_as(
        "SELECT w.updated_at, c.holder_work_id = w.id AND c.holder_fence = w.fence
                AND c.holder_token = w.token, w.lease_owner
         FROM public.consignment_lookup_work w CROSS JOIN public.consignment_lookup_control c
         WHERE w.id = $1",
    )
    .bind(chosen.fence().work_id())
    .fetch_one(fixture.pool())
    .await
    .unwrap();
    assert!(same_holder);
    assert_eq!(owner, "fake-worker");
    assert_eq!(permit.next_allowed_at, at + TimeDelta::seconds(1));
    assert_eq!(chosen.deadline(), at + TimeDelta::seconds(30));
    assert_eq!(chosen.lease_until(), chosen.deadline() + TimeDelta::seconds(5));
    fixture.allow_now().await;
    assert!(fixture.claim(1).await.is_none(), "仍持有全局许可");
}

#[tokio::test]
async fn postgres_consignment_work_claim_returns_registered_aad_and_envelope() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let claim = fixture.claim(1).await.unwrap();
    assert_eq!(claim.fence().listing_id(), LISTING);
    let credential = claim.credential().unwrap();
    assert_ne!(REGISTERED_AAD, LISTING);
    assert_eq!(credential.aad_equipment_id().as_uuid(), REGISTERED_AAD);
    let registered = EquipmentKeyEnvelope::new(
        vec![0x33; 32],
        vec![0x44; 12],
        "fake-data-key".into(),
        "ab".repeat(32),
    )
    .unwrap();
    assert_eq!(credential.envelope(), &registered);
}

#[tokio::test]
async fn postgres_consignment_work_interval_and_cooldown() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let first = fixture.claim(1).await.unwrap();
    assert_eq!(
        fixture.work().abandon(first.fence(), LookupAbandonment::RequestFinished).await.unwrap(),
        LookupLeaseUpdate::Applied(()),
    );
    assert!(fixture.claim(1).await.is_none(), "原最小间隔应保留");
    fixture.allow_now().await;
    let cooldown: Timestamp = sqlx::query_scalar(
        "UPDATE public.consignment_lookup_control SET cooldown_until = statement_timestamp() + interval '1 minute'
         WHERE id = 1 RETURNING cooldown_until",
    ).fetch_one(fixture.pool()).await.unwrap();
    let before = fixture.work_snapshot().await;
    assert!(fixture.claim(1).await.is_none(), "冷却期间不可领取");
    assert!(before == fixture.work_snapshot().await);
    assert_eq!(fixture.work().permit_status().await.unwrap().cooldown_until, Some(cooldown));
    sqlx::query("UPDATE public.consignment_lookup_control SET cooldown_until = statement_timestamp() WHERE id = 1")
        .execute(fixture.pool()).await.unwrap();
    assert_eq!(fixture.claim(1).await.unwrap().fence().work_id(), OTHER_WORK);
}

#[tokio::test]
async fn postgres_consignment_work_pause_and_epoch_gates() {
    let fixture = Fixture::new().await;
    assert!(fixture.claim(1).await.is_none());
    assert_eq!(fixture.attempts().await, 0);

    fixture.db.sync_admission(true).await.unwrap();
    assert!(fixture.claim(2).await.is_none());
    assert_eq!(fixture.attempts().await, 0);

    switch(&fixture, 0, true, None).await;
    assert!(fixture.claim(1).await.is_none());
    assert_eq!(fixture.attempts().await, 0);
    switch(&fixture, 1, false, None).await;
    let claim = fixture.claim(1).await.unwrap();
    assert_eq!(claim.attempt(), 1);
    assert_eq!(
        fixture.work().confirm_dispatch(&claim, false).await.unwrap(),
        LookupDispatch::Allowed { deadline: claim.deadline() },
    );
    assert_eq!(
        fixture.work().confirm_dispatch(&claim, true).await.unwrap(),
        LookupDispatch::Paused
    );
    switch(&fixture, 2, true, None).await;
    assert_eq!(
        fixture.work().confirm_dispatch(&claim, false).await.unwrap(),
        LookupDispatch::Paused
    );
    assert_eq!(fixture.attempts().await, 1);
    assert_eq!(
        fixture.work().abandon(claim.fence(), LookupAbandonment::NotSent).await.unwrap(),
        LookupLeaseUpdate::Applied(()),
    );
    let queued: (String, i16, bool) = sqlx::query_as(
        "SELECT state, attempt_count, next_attempt_at = updated_at
         FROM public.consignment_lookup_work WHERE id = $1",
    )
    .bind(claim.fence().work_id())
    .fetch_one(fixture.pool())
    .await
    .unwrap();
    assert_eq!(queued, ("queued".into(), 1, true));
}

#[tokio::test]
async fn postgres_consignment_work_timed_pause_expires_without_a_new_event() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let expires: Timestamp = sqlx::query_scalar("SELECT clock_timestamp() + interval '3 seconds'")
        .fetch_one(fixture.pool())
        .await
        .unwrap();
    switch(&fixture, 0, true, Some(expires)).await;
    let before = fixture.work_snapshot().await;
    assert!(fixture.claim(1).await.is_none(), "暂停到期前不可领取");
    assert!(before == fixture.work_snapshot().await);
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT clock_timestamp() < $1")
            .bind(expires)
            .fetch_one(fixture.pool())
            .await
            .unwrap()
    );

    sqlx::query("SELECT pg_sleep_until($1)").bind(expires).execute(fixture.pool()).await.unwrap();
    let claim = fixture.claim(1).await.expect("原暂停事实到期后可领取");
    assert_eq!(claim.attempt(), 1);
    assert_eq!(fixture.attempts().await, 1);
    assert_eq!(
        fixture.work().confirm_dispatch(&claim, false).await.unwrap(),
        LookupDispatch::Allowed { deadline: claim.deadline() }
    );
}

#[tokio::test]
async fn postgres_consignment_work_claim_serializes_with_pause_writer() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let mut pause_writer = fixture.owner.postgres_pool().begin().await.unwrap();
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('runtime_switch:external_access_paused',0))",
    )
    .execute(&mut *pause_writer)
    .await
    .unwrap();
    let backend: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *pause_writer).await.unwrap();
    let worker = fixture.work();
    let claim = tokio::spawn(async move { worker.claim(request(1, 1000)).await });
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1
                 FROM pg_locks owner JOIN pg_locks waiter USING (locktype, database, classid, objid, objsubid)
                 WHERE owner.pid = $1 AND owner.locktype = 'advisory' AND owner.granted AND NOT waiter.granted)",
            ).bind(backend).fetch_one(fixture.owner.postgres_pool()).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("领取应等待暂停名称锁");
    sqlx::query(
        "WITH audit AS (
             INSERT INTO public.audit_records (id, actor_type, actor_id, action, entity_type, entity_id, result, source)
             VALUES ($1, 'admin', 'fake-operator', 'runtime_switch.set', 'runtime_switch', 'external_access_paused', 'success', 'admin')
             RETURNING id, occurred_at
         )
         INSERT INTO public.runtime_switch_events
           (command_id, digest_version, request_digest, switch_name, revision, expected_revision, operation, value,
            actor_id, entrypoint, reason, audit_id, result_payload, recorded_at)
         SELECT $2, 1, decode(repeat('11', 32), 'hex'), 'external_access_paused', 1, 0, 'set', 'true',
                'fake-operator', 'admin', '测试外部暂停', id, '{}'::jsonb, occurred_at
         FROM audit",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(&mut *pause_writer)
    .await
    .unwrap();
    pause_writer.commit().await.unwrap();
    assert!(claim.await.unwrap().unwrap().is_none());
    assert_eq!(fixture.attempts().await, 0);
}

#[tokio::test]
async fn postgres_consignment_work_claim_ignores_listing_write_lock() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let mut listing_writer = fixture.owner.postgres_pool().begin().await.unwrap();
    sqlx::query("SELECT id FROM public.consignment_listings WHERE id = $1 FOR UPDATE")
        .bind(LISTING)
        .fetch_one(&mut *listing_writer)
        .await
        .unwrap();
    let claimed = tokio::time::timeout(Duration::from_millis(800), fixture.claim(1))
        .await
        .expect("领取不应等待挂单写锁")
        .unwrap();
    assert_eq!(claimed.fence().work_id(), WORK);
    listing_writer.rollback().await.unwrap();
}

#[tokio::test]
async fn postgres_consignment_work_obeys_qualification_and_listing_deadline() {
    let fixture = Fixture::deadline_seconds(20.0).await;
    let expires_at = fixture.expires().await;
    fixture.db.sync_admission(true).await.unwrap();
    let claim = fixture.claim(1).await.unwrap();
    assert_eq!(claim.deadline(), expires_at);
    assert_eq!(claim.lease_until(), expires_at + TimeDelta::seconds(5));
    sqlx::query("UPDATE public.users SET status = 'blacklisted' WHERE id = $1")
        .bind(SELLER)
        .execute(fixture.owner.postgres_pool())
        .await
        .unwrap();
    assert_eq!(fixture.work().confirm_dispatch(&claim, false).await.unwrap(), LookupDispatch::Lost);
    assert_eq!(fixture.work().renew(claim.fence()).await.unwrap(), LookupLeaseUpdate::Lost);
    assert_eq!(
        fixture.work().abandon(claim.fence(), LookupAbandonment::NotSent).await.unwrap(),
        LookupLeaseUpdate::Applied(()),
    );
    fixture.allow_now().await;
    assert!(fixture.claim(1).await.is_none());
    assert_eq!(fixture.attempts().await, 1);
}

#[tokio::test]
async fn postgres_consignment_work_claim_requires_due_and_remaining_budget() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    sqlx::query(
        "UPDATE public.consignment_lookup_work
         SET attempt_count = CASE WHEN id = $1 THEN 6 ELSE 0 END,
             next_attempt_at = CASE WHEN id = $1 THEN created_at ELSE statement_timestamp() + interval '1 hour' END",
    )
    .bind(WORK)
    .execute(fixture.pool()).await.unwrap();
    let before = fixture.work_snapshot().await;
    assert!(fixture.claim(1).await.is_none());
    assert!(before == fixture.work_snapshot().await);
    sqlx::query(
        "UPDATE public.consignment_lookup_work SET next_attempt_at = created_at WHERE id = $1",
    )
    .bind(OTHER_WORK)
    .execute(fixture.pool())
    .await
    .unwrap();
    assert_eq!(fixture.claim(1).await.unwrap().fence().work_id(), OTHER_WORK);
}

#[tokio::test]
async fn postgres_consignment_work_missing_credential_is_safe() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    sqlx::query(
        "UPDATE public.consignment_registration_secrets
         SET ciphertext = NULL, nonce = NULL, encryption_key_id = NULL, fingerprint = NULL,
             updated_at = statement_timestamp(), retired_at = statement_timestamp()
         WHERE listing_id = $1",
    )
    .bind(LISTING)
    .execute(fixture.pool())
    .await
    .unwrap();
    let claim = fixture.claim(1).await.unwrap();
    assert_eq!(claim.fence().work_id(), WORK);
    assert!(claim.credential().is_err());
    assert_eq!(fixture.attempts().await, 1);
}

#[tokio::test]
async fn postgres_consignment_work_claim_increments_persisted_attempt() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    sqlx::query(
        "UPDATE public.consignment_lookup_work SET attempt_count = 4, fence = 9 WHERE id = $1",
    )
    .bind(WORK)
    .execute(fixture.pool())
    .await
    .unwrap();
    let claim = fixture.claim(1).await.unwrap();
    assert_eq!(claim.fence().work_id(), WORK);
    assert_eq!(claim.fence().fence(), 10);
    assert_eq!(claim.attempt(), 5);
    assert_eq!(fixture.attempts().await, 5);
}

async fn switch(fixture: &Fixture, revision: i64, paused: bool, expires_at: Option<Timestamp>) {
    fixture
        .db
        .write(RuntimeSwitchCommand {
            command_id: Uuid::new_v4(),
            name: RuntimeSwitchName::ExternalAccessPaused,
            expected_revision: revision,
            operation: if paused {
                RuntimeSwitchOperation::Set
            } else {
                RuntimeSwitchOperation::Clear
            },
            value: paused.then_some(json!(true)),
            expires_at,
            expired_command_id: None,
            actor_id: "fake-operator".into(),
            operator_identity: None,
            entrypoint: "admin".into(),
            reason: "测试外部暂停".into(),
        })
        .await
        .unwrap();
}
