//! 验证解析结果、持久提示和暂停通知只影响调度。

use std::sync::atomic::Ordering;

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingCommandOutcome, ListingCommandRejection,
    ListingLookupEffect, ListingWorkerReadiness, LookupTransientReason, UnrecordedListingRejection,
};
use auction_observability::WorkerResult;
use tokio::time::Instant;

use super::fixture::{ClaimStep, EPOCH, Fixture, advance_ms, settle, time};
use crate::worker::consignment::ConsignmentLoop;

#[tokio::test(start_paused = true)]
async fn idle_uses_future_hint_then_poll_for_past_hint() {
    let fixture = Fixture::new();
    fixture.work.wake_at.store(2, Ordering::Relaxed);
    let started = Instant::now();
    let running = fixture.start().await;
    settle().await;
    advance_ms(1_999).await;
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 1);
    advance_ms(1).await;
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 2);
    advance_ms(4_999).await;
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 2);
    advance_ms(1).await;
    assert_eq!(
        fixture
            .work
            .claims
            .lock()
            .unwrap()
            .iter()
            .map(|(at, _, _)| (*at - started).as_millis())
            .collect::<Vec<_>>(),
        [0, 2_000, 7_000]
    );
    running.finish().await;
}

#[tokio::test(start_paused = true)]
async fn idle_notification_wakes_before_the_poll_and_long_hint() {
    let fixture = Fixture::new();
    fixture.work.wake_at.store(60, Ordering::Relaxed);
    let running = fixture.start().await;
    settle().await;
    advance_ms(1_000).await;
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 1);
    fixture.wakeup.notify_one();
    settle().await;
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 2);
    advance_ms(4_999).await;
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 2);
    advance_ms(1).await;
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 3);
    running.finish().await;
}

#[tokio::test(start_paused = true)]
async fn idle_one_hour_only_claims_once_per_poll() {
    let fixture = Fixture::new();
    let running = fixture.start().await;
    settle().await;
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 1);
    for _ in 0..3_600 {
        advance_ms(1_000).await;
    }
    assert_eq!(fixture.work.claims.lock().unwrap().len(), 721);
    assert_eq!(fixture.work.permit_reads.load(Ordering::Relaxed), 721);
    assert_eq!(fixture.equipment.requests.load(Ordering::Relaxed), 0);
    running.finish().await;
}

#[tokio::test(start_paused = true)]
async fn pause_never_claims_and_resumes_at_the_bounded_reread() {
    let fixture = Fixture::new();
    fixture.paused.store(true, Ordering::Relaxed);
    let running = fixture.start().await;
    settle().await;
    for _ in 0..4 {
        fixture.wakeup.notify_one();
        advance_ms(5_000).await;
    }
    assert!(fixture.work.claims.lock().unwrap().is_empty());
    assert_eq!(fixture.work.permit_reads.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.equipment.requests.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.work.reaps.load(Ordering::Relaxed), 1);
    fixture.paused.store(false, Ordering::Relaxed);
    advance_ms(4_999).await;
    assert!(fixture.work.claims.lock().unwrap().is_empty());
    advance_ms(1).await;
    assert!(!fixture.work.claims.lock().unwrap().is_empty());
    running.finish().await;
}

async fn immediate_retry(
    step: Result<ListingCommandOutcome<ListingLookupEffect>, ConsignmentError>,
    expected_result: WorkerResult,
) {
    let fixture = Fixture::new();
    let submitted = usize::from(step.is_ok());
    let claim = match step {
        Ok(observation) => {
            fixture.commands.apply.lock().unwrap().push_back(Ok(observation));
            ClaimStep::Claim
        }
        Err(error) => ClaimStep::Error(error),
    };
    fixture.work.steps.lock().unwrap().push_back(claim);
    let started = Instant::now();
    let running = fixture.start().await;
    settle().await;
    {
        let calls = fixture.work.claims.lock().unwrap();
        let [first, second] = calls.as_slice() else {
            panic!("已结束的解析一轮须立即再次领取");
        };
        assert_eq!((first.0, first.1), (started, EPOCH));
        assert_eq!((second.0, second.1), (started, EPOCH));
        assert_ne!(first.2, second.2);
        assert!(!first.2.is_nil() && !second.2.is_nil());
    }
    assert_eq!(fixture.equipment.requests.load(Ordering::Relaxed), submitted);
    assert_eq!(fixture.commands.applied.load(Ordering::Relaxed), submitted);
    assert_eq!(fixture.work.ready.lock().unwrap()[0].2, ListingWorkerReadiness::Ready);
    assert_eq!(
        running
            .observations
            .lock()
            .unwrap()
            .iter()
            .filter(|(kind, _, _)| *kind == ConsignmentLoop::Resolve)
            .map(|(_, result, failed)| (*result, *failed))
            .collect::<Vec<_>>(),
        [(expected_result, 0), (WorkerResult::Success, 0)]
    );
    running.finish().await;
}

#[tokio::test(start_paused = true)]
async fn applied_immediately_runs_next_round_without_waiting_for_item_retry() {
    immediate_retry(
        Ok(ListingCommandOutcome::Accepted(ListingLookupEffect::Rescheduled {
            next_attempt_at: time(300),
            reason: LookupTransientReason::SourceUnavailable,
        })),
        WorkerResult::Success,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn lost_lease_is_classified_and_immediately_runs_next_round() {
    immediate_retry(Err(ConsignmentError::LeaseLost), WorkerResult::Canceled).await;
    immediate_retry(
        Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Unrecorded(
            UnrecordedListingRejection::LeaseLost,
        ))),
        WorkerResult::Canceled,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn committed_close_immediately_runs_next_round() {
    immediate_retry(
        Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(
            ListingBusinessRejection::ListingNotActive,
        ))),
        WorkerResult::Success,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn dependency_backoff_caps_and_resets_after_a_healthy_round() {
    let fixture = Fixture::new();
    let unavailable = ClaimStep::Error(ConsignmentError::Unavailable);
    fixture.work.steps.lock().unwrap().extend([
        unavailable,
        unavailable,
        unavailable,
        unavailable,
        unavailable,
        unavailable,
        unavailable,
        ClaimStep::Idle,
        unavailable,
        ClaimStep::Idle,
    ]);
    let started = Instant::now();
    let running = fixture.start().await;
    settle().await;
    for (index, ms) in
        [1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000, 5_000, 1_000].into_iter().enumerate()
    {
        if index < 6 {
            fixture.wakeup.notify_one();
        }
        advance_ms(ms - 1).await;
        assert_eq!(fixture.work.claims.lock().unwrap().len(), index + 1, "阶段 {index}");
        if index == 6 {
            // 退避不消费通知。清掉合并的一次通知，让下一轮空闲测到完整轮询间隔。
            tokio::select! {
                biased;
                () = fixture.wakeup.notified() => {}
                () = std::future::ready(()) => panic!("退避不应消费通知"),
            }
        }
        advance_ms(1).await;
        assert_eq!(fixture.work.claims.lock().unwrap().len(), index + 2, "阶段 {index}");
    }
    assert_eq!(
        fixture
            .work
            .claims
            .lock()
            .unwrap()
            .iter()
            .map(|(at, _, _)| (*at - started).as_secs())
            .collect::<Vec<_>>(),
        [0, 1, 3, 7, 15, 31, 61, 91, 96, 97]
    );
    running.finish().await;
}
