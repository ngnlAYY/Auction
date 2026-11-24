//! 验证启动、停机、维护和持久就绪心跳的生命周期。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ListingAutoCloseEffect, ListingCommandOutcome, ListingLookupEffect,
    ListingMaintenanceService, ListingWorkerReadiness, LookupAbandonment, LookupTransientReason,
};
use auction_application::core::diagnostics::SafeErrorKind;
use auction_observability::{WorkerLoop, WorkerResult};
use tokio::sync::{oneshot, watch};
use tokio::time::{Instant, timeout};

use super::fixture::{
    ClaimStep, EPOCH, Fixture, Running, advance_ms, candidate, page, settle, time,
};
use crate::WorkerError;
use crate::worker::consignment::{
    ConsignmentDependencies, ConsignmentLoop, ConsignmentWorker, observe_worker_iteration,
};
use crate::worker::runtime::test_fixture::{PausedClock, RecordingMetrics, worker};

#[tokio::test(start_paused = true)]
async fn persistent_runner_supervises_consignment_and_awaits_its_drain() {
    let fixture = Fixture::new();
    fixture.work.steps.lock().unwrap().extend([ClaimStep::Claim, ClaimStep::Claim]);
    fixture.equipment.gate.blocked.store(true, Ordering::Relaxed);
    fixture.work.abandon_gate.blocked.store(true, Ordering::Relaxed);
    let candidates = [candidate(1), candidate(2)];
    fixture.scans.pages.lock().unwrap().push_back(page(candidates.to_vec(), None));
    fixture
        .commands
        .close
        .lock()
        .unwrap()
        .extend(candidates.map(|_| Err(ConsignmentError::Unavailable)));
    let consignment = fixture.prepare(true, async { Ok(false) }).await.unwrap();
    let mut worker = worker(Arc::new(PausedClock::new(time(0))));
    let metrics = Arc::new(RecordingMetrics::default());
    worker.metrics = metrics.clone();
    let control = worker.control.clone();
    let (stop, stop_requested) = oneshot::channel();
    let runner = tokio::spawn(worker.run_with_consignment_until_shutdown(consignment, async {
        stop_requested.await.unwrap();
    }));
    settle().await;

    assert!(control.is_ready());
    assert!(fixture.equipment.gate.active.load(Ordering::Relaxed));
    assert_eq!(fixture.work.ready.lock().unwrap().len(), 1);
    assert!(
        metrics.iterations().contains(&(WorkerLoop::ConsignmentReadiness, WorkerResult::Success))
    );
    assert!(
        metrics.iterations().contains(&(WorkerLoop::ConsignmentMaintenance, WorkerResult::Success))
    );
    assert_eq!(*fixture.commands.closed.lock().unwrap(), candidates);
    assert_eq!(
        metrics
            .unpersisted()
            .into_iter()
            .filter(|(kind, _)| *kind == WorkerLoop::ConsignmentMaintenance)
            .collect::<Vec<_>>(),
        [(WorkerLoop::ConsignmentMaintenance, SafeErrorKind::CONSIGNMENT_AUTO_CLOSE_FAILED); 2],
    );

    stop.send(()).unwrap();
    settle().await;
    assert!(control.is_draining());
    assert_eq!(*fixture.work.abandons.lock().unwrap(), [LookupAbandonment::MayBeInFlight]);
    assert!(fixture.work.abandon_gate.active.load(Ordering::Relaxed));
    assert!(!runner.is_finished());

    fixture.work.abandon_gate.resume.notify_one();
    runner.await.unwrap().unwrap();
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 1);
    assert!(
        metrics.iterations().contains(&(WorkerLoop::ConsignmentResolve, WorkerResult::Canceled))
    );
}

#[test]
fn consignment_metrics_count_each_failed_candidate() {
    let metrics = RecordingMetrics::default();
    observe_worker_iteration(
        &metrics,
        ConsignmentLoop::Maintenance,
        WorkerResult::Success,
        Duration::ZERO,
        0,
    );
    assert!(metrics.unpersisted().is_empty());
    observe_worker_iteration(
        &metrics,
        ConsignmentLoop::Maintenance,
        WorkerResult::Success,
        Duration::ZERO,
        2,
    );
    assert_eq!(
        metrics.unpersisted(),
        [(WorkerLoop::ConsignmentMaintenance, SafeErrorKind::CONSIGNMENT_AUTO_CLOSE_FAILED); 2],
    );
}

#[tokio::test(start_paused = true)]
async fn persistent_runner_drains_when_consignment_task_panics() {
    let fixture = Fixture::new();
    // 未给观察提交设置回执，替身在该边界 panic。
    fixture.work.steps.lock().unwrap().push_back(ClaimStep::Claim);
    let consignment = fixture.prepare(true, async { Ok(false) }).await.unwrap();
    let worker = worker(Arc::new(PausedClock::new(time(0))));
    let control = worker.control.clone();
    let result = timeout(
        Duration::from_secs(3),
        worker.run_with_consignment_until_shutdown(consignment, std::future::pending()),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(WorkerError::ConsignmentTaskJoin(error)) if error.is_panic()));
    assert!(control.is_draining());
}

#[tokio::test(start_paused = true)]
async fn failed_startup_sync_stops_before_loops_and_stock_probe() {
    let fixture = Fixture::new();
    for enabled in [true, false] {
        *fixture.scans.sync_error.lock().unwrap() = Some(ConsignmentError::OutcomeUnknown);
        let result = fixture.prepare(enabled, async { panic!("同步未确认不应探测存量") }).await;
        assert!(matches!(result, Err(ConsignmentError::OutcomeUnknown)));
    }
    assert_eq!(*fixture.scans.syncs.lock().unwrap(), [true, false]);
    assert!(fixture.work.claims.lock().unwrap().is_empty());
    assert!(fixture.scans.scans.lock().unwrap().is_empty());
    assert!(fixture.work.ready.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn incapable_artifact_rejects_enabled_startup_without_sync() {
    let fixture = Fixture::new();
    let result = fixture
        .prepare_with_capability(true, false, async {
            panic!("制品不支持时不应探测存量")
        })
        .await;
    assert_eq!(fixture.scans.syncs.lock().unwrap().as_slice(), &[] as &[bool]);
    assert!(matches!(result, Err(ConsignmentError::Disabled)));

    let result = ConsignmentWorker::prepare(
        true,
        false,
        ListingMaintenanceService::new(fixture.scans.clone(), fixture.commands.clone()),
        None,
        async { panic!("制品不支持时不应探测存量") },
    )
    .await;
    assert!(matches!(result, Err(ConsignmentError::Disabled)));
    assert!(fixture.scans.syncs.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn disabled_empty_only_syncs_and_checks_stock_once() {
    let fixture = Fixture::new();
    let empty = fixture
        .prepare(false, async {
            assert_eq!(*fixture.scans.syncs.lock().unwrap(), [false]);
            Ok(false)
        })
        .await
        .unwrap();
    assert!(empty.is_none());
    advance_ms(90_000).await;
    assert!(fixture.scans.scans.lock().unwrap().is_empty());
    assert!(fixture.work.claims.lock().unwrap().is_empty());
    assert!(fixture.work.ready.lock().unwrap().is_empty());

    let unknown = fixture.prepare(false, async { Err(ConsignmentError::Unavailable) }).await;
    assert!(matches!(unknown, Err(ConsignmentError::Unavailable)));
    assert_eq!(*fixture.scans.syncs.lock().unwrap(), [false, false]);
}

#[tokio::test(start_paused = true)]
async fn maintenance_follows_pages_after_item_failure() {
    let fixture = Fixture::new();
    let [expiry, collision, ineligible, disabled] = [1, 2, 3, 4].map(candidate);
    fixture.scans.pages.lock().unwrap().extend([
        page(vec![expiry, collision], Some(collision.listing_id)),
        page(vec![ineligible], Some(ineligible.listing_id)),
        page(vec![disabled], None),
        page(vec![ineligible], None),
    ]);
    fixture.commands.close.lock().unwrap().extend([
        Ok(ListingCommandOutcome::Accepted(ListingAutoCloseEffect::Unchanged)),
        Ok(ListingCommandOutcome::Accepted(ListingAutoCloseEffect::ConflictsRecorded)),
        Err(ConsignmentError::Unavailable),
    ]);
    let worker = fixture.prepare(false, async { Ok(true) }).await.unwrap().unwrap();
    let running = Running::spawn(worker);
    settle().await;
    assert_eq!(fixture.scans.scans.lock().unwrap().len(), 3);
    assert_eq!(*fixture.commands.closed.lock().unwrap(), [expiry, collision, ineligible, disabled]);
    advance_ms(29_999).await;
    assert_eq!(fixture.scans.scans.lock().unwrap().len(), 3);
    advance_ms(1).await;
    assert_eq!(
        *fixture.commands.closed.lock().unwrap(),
        [expiry, collision, ineligible, disabled, ineligible]
    );
    assert_eq!(fixture.scans.scans.lock().unwrap().len(), 4);
    assert!(fixture.work.claims.lock().unwrap().is_empty());
    assert!(fixture.work.ready.lock().unwrap().is_empty());
    assert_eq!(fixture.work.reaps.load(Ordering::Relaxed), 0);
    assert_eq!(
        *running.observations.lock().unwrap(),
        [
            (ConsignmentLoop::Maintenance, WorkerResult::Success, 0),
            (ConsignmentLoop::Maintenance, WorkerResult::Success, 1),
            (ConsignmentLoop::Maintenance, WorkerResult::Success, 0),
            (ConsignmentLoop::Maintenance, WorkerResult::Success, 0),
        ]
    );
    running.finish().await;
}

#[tokio::test(start_paused = true)]
async fn maintenance_reaches_later_listing_on_every_wrap_despite_persistent_failure() {
    let fixture = Fixture::new();
    let [ineligible, disabled] = [3, 4].map(candidate);
    for _ in 0..3 {
        fixture.scans.pages.lock().unwrap().push_back(page(vec![ineligible, disabled], None));
    }
    *fixture.commands.fail_close_listing.lock().unwrap() = Some(ineligible.listing_id);
    let worker = fixture.prepare(false, async { Ok(true) }).await.unwrap().unwrap();
    let running = Running::spawn(worker);
    settle().await;
    let first_round = fixture.commands.closed.lock().unwrap().clone();
    advance_ms(30_000).await;
    advance_ms(30_000).await;
    assert_eq!(
        *fixture.commands.closed.lock().unwrap(),
        [ineligible, disabled, ineligible, disabled, ineligible, disabled]
    );
    assert_eq!(first_round, [ineligible, disabled]);
    assert_eq!(fixture.scans.scans.lock().unwrap().len(), 3);
    assert_eq!(
        *running.observations.lock().unwrap(),
        [(ConsignmentLoop::Maintenance, WorkerResult::Success, 1); 3]
    );
    running.finish().await;
}

#[tokio::test(start_paused = true)]
async fn reaping_failure_does_not_prevent_sweeping_or_next_reap() {
    let fixture = Fixture::new();
    fixture.work.reap_fails.store(true, Ordering::Relaxed);
    let running = fixture.start().await;
    settle().await;
    assert_eq!(fixture.scans.scans.lock().unwrap().len(), 1);
    assert_eq!(fixture.work.reaps.load(Ordering::Relaxed), 1);
    fixture.work.reap_fails.store(false, Ordering::Relaxed);
    advance_ms(30_000).await;
    assert_eq!(fixture.scans.scans.lock().unwrap().len(), 2);
    assert_eq!(fixture.work.reaps.load(Ordering::Relaxed), 2);
    running.finish().await;
}

#[tokio::test(start_paused = true)]
async fn already_draining_does_not_start_any_work() {
    let fixture = Fixture::new();
    let worker = fixture.prepare(true, async { Ok(false) }).await.unwrap().unwrap();
    let (_sender, receiver) = watch::channel(true);
    worker.run(receiver, |_, _, _, _| panic!("排空后不应开始新一轮")).await;
    assert!(fixture.work.claims.lock().unwrap().is_empty());
    assert!(fixture.scans.scans.lock().unwrap().is_empty());
    assert_eq!(fixture.work.reaps.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.readiness.calls.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn shutdown_waits_for_observed_apply_and_maintenance_then_stops() {
    let fixture = Fixture::new();
    fixture.work.steps.lock().unwrap().extend([ClaimStep::Claim, ClaimStep::Claim]);
    fixture.commands.apply_gate.blocked.store(true, Ordering::Relaxed);
    fixture.commands.apply.lock().unwrap().push_back(Ok(ListingCommandOutcome::Accepted(
        ListingLookupEffect::Rescheduled {
            reason: LookupTransientReason::SourceUnavailable,
            next_attempt_at: time(300),
        },
    )));
    let first = candidate(1);
    fixture
        .scans
        .pages
        .lock()
        .unwrap()
        .extend([page(vec![first], Some(first.listing_id)), page(vec![candidate(2)], None)]);
    fixture.commands.close_gate.blocked.store(true, Ordering::Relaxed);
    let mut running = fixture.start().await;
    settle().await;
    assert!(fixture.commands.apply_gate.active.load(Ordering::Relaxed));
    assert!(fixture.commands.close_gate.active.load(Ordering::Relaxed));
    running.drain.send_replace(true);
    advance_ms(20_000).await;
    assert!(running.tasks.try_join_next().is_none());
    assert!(fixture.commands.apply_gate.active.load(Ordering::Relaxed));
    assert!(fixture.commands.close_gate.active.load(Ordering::Relaxed));
    fixture.commands.apply_gate.resume.notify_one();
    settle().await;
    assert!(running.tasks.try_join_next().is_none());
    fixture.commands.close_gate.resume.notify_one();
    settle().await;
    assert!(matches!(running.tasks.try_join_next(), Some(Ok(()))));
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 1);
    assert_eq!(fixture.scans.scans.lock().unwrap().len(), 1);
    assert_eq!(fixture.work.ready.lock().unwrap().len(), 1);
    assert!(fixture.work.abandons.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn shutdown_during_dispatch_check_leaves_without_sending() {
    let fixture = Fixture::new();
    fixture.work.steps.lock().unwrap().extend([ClaimStep::Claim]);
    fixture.work.dispatch_gate.blocked.store(true, Ordering::Relaxed);
    let running = fixture.start().await;
    settle().await;
    assert!(fixture.work.dispatch_gate.active.load(Ordering::Relaxed));
    running.drain.send_replace(true);
    fixture.work.dispatch_gate.resume.notify_one();
    running.finish().await;
    assert_eq!(*fixture.work.abandons.lock().unwrap(), [LookupAbandonment::NotSent]);
    assert_eq!(fixture.equipment.requests.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.commands.applied.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn shutdown_in_flight_awaits_may_be_in_flight_abandonment() {
    let fixture = Fixture::new();
    fixture.work.steps.lock().unwrap().extend([ClaimStep::Claim, ClaimStep::Claim]);
    fixture.equipment.gate.blocked.store(true, Ordering::Relaxed);
    fixture.work.abandon_gate.blocked.store(true, Ordering::Relaxed);
    let mut running = fixture.start().await;
    settle().await;
    advance_ms(15_000).await;
    assert_eq!(fixture.equipment.requests.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 1);
    assert!(fixture.equipment.gate.active.load(Ordering::Relaxed));
    assert!(fixture.work.abandons.lock().unwrap().is_empty());
    running.drain.send_replace(true);
    settle().await;
    assert_eq!(*fixture.work.abandons.lock().unwrap(), [LookupAbandonment::MayBeInFlight]);
    assert!(!fixture.equipment.gate.active.load(Ordering::Relaxed));
    assert!(fixture.work.abandon_gate.active.load(Ordering::Relaxed));
    assert!(running.tasks.try_join_next().is_none());
    fixture.work.abandon_gate.resume.notify_one();
    settle().await;
    assert!(matches!(running.tasks.try_join_next(), Some(Ok(()))));
    assert_eq!(fixture.commands.applied.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn stalled_abandonment_is_left_to_the_outer_drain_deadline() {
    let fixture = Fixture::new();
    fixture.work.steps.lock().unwrap().push_back(ClaimStep::Claim);
    fixture.equipment.gate.blocked.store(true, Ordering::Relaxed);
    fixture.work.abandon_gate.blocked.store(true, Ordering::Relaxed);
    let mut running = fixture.start().await;
    settle().await;
    running.drain.send_replace(true);
    settle().await;
    let started = Instant::now();
    assert!(timeout(Duration::from_secs(2), running.tasks.join_next()).await.is_err());
    assert!(Instant::now() - started >= Duration::from_secs(2));
    assert_eq!(*fixture.work.abandons.lock().unwrap(), [LookupAbandonment::MayBeInFlight]);
    assert!(fixture.work.abandon_gate.active.load(Ordering::Relaxed));
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 1);
    drop(running);
    settle().await;
    assert!(!fixture.work.abandon_gate.active.load(Ordering::Relaxed));
    assert_eq!(fixture.commands.applied.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn heartbeat_needs_every_dependency_and_retries_failed_db_write_at_ten_seconds() {
    let fixture = Fixture::new();
    let all = *fixture.readiness.deps.lock().unwrap();
    let started = Instant::now();
    let running = fixture.start().await;
    settle().await;
    assert_eq!(
        *fixture.work.ready.lock().unwrap(),
        [(started, EPOCH, ListingWorkerReadiness::Ready)]
    );
    let mut renewals = Vec::new();
    for deps in [
        ConsignmentDependencies { database: false, ..all },
        ConsignmentDependencies { schema: false, ..all },
        ConsignmentDependencies { opener: false, ..all },
        ConsignmentDependencies { current_keyring: false, ..all },
        ConsignmentDependencies { lookup: false, ..all },
    ] {
        *fixture.readiness.deps.lock().unwrap() = deps;
        advance_ms(10_000).await;
        renewals.push(fixture.work.ready.lock().unwrap().len());
    }
    assert_eq!(renewals, [1; 5], "分别缺数据库、schema、opener、当前 keyring、lookup");
    assert_eq!(fixture.readiness.calls.load(Ordering::Relaxed), 6);
    *fixture.readiness.deps.lock().unwrap() = all;
    *fixture.readiness.error.lock().unwrap() = Some(ConsignmentError::Unavailable);
    advance_ms(9_999).await;
    assert_eq!(fixture.work.ready.lock().unwrap().len(), 1);
    advance_ms(1).await;
    assert_eq!(fixture.readiness.calls.load(Ordering::Relaxed), 7);
    assert_eq!(fixture.work.ready.lock().unwrap().len(), 1, "探针返回错误时不能续期");
    assert_eq!(
        running
            .observations
            .lock()
            .unwrap()
            .iter()
            .rfind(|(kind, _, _)| *kind == ConsignmentLoop::Readiness),
        Some(&(ConsignmentLoop::Readiness, WorkerResult::Error, 0)),
    );
    *fixture.readiness.error.lock().unwrap() = None;
    advance_ms(10_000).await;
    assert_eq!(fixture.work.ready.lock().unwrap().len(), 2);
    fixture.work.ready_fails.store(true, Ordering::Relaxed);
    advance_ms(10_000).await;
    assert_eq!(fixture.work.ready.lock().unwrap().len(), 3);
    assert_eq!(
        running
            .observations
            .lock()
            .unwrap()
            .iter()
            .rfind(|(kind, _, _)| *kind == ConsignmentLoop::Readiness),
        Some(&(ConsignmentLoop::Readiness, WorkerResult::Error, 0)),
    );
    fixture.work.ready_fails.store(false, Ordering::Relaxed);
    advance_ms(10_000).await;
    assert_eq!(
        fixture
            .work
            .ready
            .lock()
            .unwrap()
            .iter()
            .map(|(at, epoch, status)| { ((*at - started).as_secs(), *epoch, *status) })
            .collect::<Vec<_>>(),
        [
            (0, EPOCH, ListingWorkerReadiness::Ready),
            (70, EPOCH, ListingWorkerReadiness::Ready),
            (80, EPOCH, ListingWorkerReadiness::Ready),
            (90, EPOCH, ListingWorkerReadiness::Ready),
        ]
    );
    running.finish().await;
}

#[tokio::test(start_paused = true)]
async fn shutdown_during_readiness_check_does_not_renew() {
    let fixture = Fixture::new();
    fixture.readiness.gate.blocked.store(true, Ordering::Relaxed);
    let mut running = fixture.start().await;
    settle().await;
    advance_ms(2_000).await;
    assert!(fixture.readiness.gate.active.load(Ordering::Relaxed));
    running.drain.send_replace(true);
    settle().await;
    assert!(matches!(running.tasks.try_join_next(), Some(Ok(()))));
    assert!(!fixture.readiness.gate.active.load(Ordering::Relaxed));
    assert!(fixture.work.ready.lock().unwrap().is_empty());
}
