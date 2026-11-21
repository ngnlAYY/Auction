//! 准入转换、跨进程同步和 Worker 就绪的真实 PostgreSQL 断言。

use auction_application::consignment::{
    ListingAdmission, ListingMaintenance, ListingWorkerReadiness, LookupDispatch,
    LookupLeaseUpdate, LookupWorkStore,
};
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use tokio::sync::Barrier;

use super::fixture::{Fixture, WORK};

#[tokio::test]
async fn postgres_consignment_work_admission_transitions() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.db.sync_admission(false).await.unwrap(), admission(1, false));
    assert_eq!(fixture.db.sync_admission(true).await.unwrap(), admission(1, true));
    assert_eq!(fixture.db.sync_admission(true).await.unwrap(), admission(1, true));
    let claim = fixture.claim(1).await.unwrap();
    let work = fixture.work();
    work.record_worker_readiness(1, ListingWorkerReadiness::Ready).await.unwrap();
    assert!(fixture.heartbeat().await.is_some());
    assert_eq!(fixture.db.sync_admission(false).await.unwrap(), admission(2, false));
    assert_eq!(fixture.heartbeat().await, None);
    assert_eq!(fixture.db.sync_admission(false).await.unwrap(), admission(2, false));
    assert_eq!(work.confirm_dispatch(&claim, false).await.unwrap(), LookupDispatch::Lost);
    assert_eq!(work.renew(claim.fence()).await.unwrap(), LookupLeaseUpdate::Lost);
    assert_eq!(work.permit_status().await.unwrap().permit_until, Some(claim.lease_until()));
    assert_eq!(fixture.db.sync_admission(true).await.unwrap(), admission(2, true));
    assert!(fixture.claim(1).await.is_none());
    assert_eq!(fixture.heartbeat().await, None);
    assert_eq!(work.confirm_dispatch(&claim, false).await.unwrap(), LookupDispatch::Lost);
    fixture.expire_lease(claim.fence()).await;
    fixture.allow_now().await;
    // 停用前遗留的队列和已过期租约交给自动清退；重开不会重领旧单。
    assert_eq!(work.reap_expired().await.unwrap(), 0);
    assert!(fixture.claim(2).await.is_none());
    assert_eq!(fixture.attempts().await, 1);
    assert_eq!(work.permit_status().await.unwrap().service_epoch, 2);
}

#[tokio::test]
async fn postgres_consignment_work_concurrent_admission_sync() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let barrier = Barrier::new(2);
    let (first, second) = tokio::join!(
        async {
            barrier.wait().await;
            fixture.db.sync_admission(false).await.unwrap()
        },
        async {
            barrier.wait().await;
            fixture.owner.sync_admission(false).await.unwrap()
        },
    );
    assert_eq!(first, admission(2, false));
    assert_eq!(second, admission(2, false));
    let observed = fixture.work().permit_status().await.unwrap();
    assert_eq!((observed.service_epoch, observed.admitting), (2, false));
}

#[tokio::test]
async fn postgres_consignment_work_readiness_rejects_stale_epoch() {
    let fixture = Fixture::new().await;
    let work = fixture.work();
    work.record_worker_readiness(1, ListingWorkerReadiness::Ready).await.unwrap();
    assert_eq!(fixture.heartbeat().await, None);
    fixture.db.sync_admission(true).await.unwrap();
    fixture.db.sync_admission(false).await.unwrap();
    fixture.db.sync_admission(true).await.unwrap();
    let before = now(fixture.pool()).await;
    work.record_worker_readiness(2, ListingWorkerReadiness::Ready).await.unwrap();
    let ready_until = fixture.heartbeat().await.unwrap();
    let after = now(fixture.pool()).await;
    assert!(
        (before + Duration::seconds(30)..=after + Duration::seconds(30)).contains(&ready_until)
    );

    // 使任何意外续期都可被精确观测，不需要等待数据库时钟跨秒。
    let fixed: Timestamp = sqlx::query_scalar(
        "UPDATE public.consignment_lookup_control SET worker_ready_until = statement_timestamp() WHERE id = 1
         RETURNING worker_ready_until",
    ).fetch_one(fixture.pool()).await.unwrap();
    work.record_worker_readiness(1, ListingWorkerReadiness::Ready).await.unwrap();
    assert_eq!(fixture.heartbeat().await, Some(fixed));
    work.record_worker_readiness(2, ListingWorkerReadiness::Unavailable).await.unwrap();
    assert_eq!(fixture.heartbeat().await, Some(fixed));
    work.record_worker_readiness(2, ListingWorkerReadiness::Ready).await.unwrap();
    assert!(fixture.heartbeat().await.unwrap() > fixed);
}

#[tokio::test]
async fn postgres_consignment_work_shutdown_preserves_original_grace() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let claim = fixture.claim(1).await.unwrap();
    sqlx::query(
        "WITH changed AS (
             UPDATE public.consignment_lookup_work SET lease_until = network_deadline + interval '1 second'
             WHERE id = $1 RETURNING lease_until
         )
         UPDATE public.consignment_lookup_control c SET permit_until = changed.lease_until FROM changed WHERE c.id = 1",
    )
    .bind(WORK)
    .execute(fixture.pool()).await.unwrap();
    assert_eq!(fixture.db.sync_admission(false).await.unwrap(), admission(2, false));
    let observed = fixture.work().permit_status().await.unwrap();
    assert_eq!(observed.permit_until, Some(claim.deadline() + Duration::seconds(5)));
}

fn admission(service_epoch: u64, admitting: bool) -> ListingAdmission {
    ListingAdmission { service_epoch, admitting }
}

async fn now(pool: &PgPool) -> Timestamp {
    sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.unwrap()
}
