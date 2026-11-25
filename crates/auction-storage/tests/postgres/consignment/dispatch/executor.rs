//! 派发 executor 端到端（W-5）：两个 executor 并发处理同一寄售义务的首轮，只有一个取得起写许可并
//! POST 一次，Sent 绑定证据落库；之后任何一轮都只得到“已终结”，零第二次 POST。
//!
//! 寄售义务的领取（履约 claim）还没有生产写端，这里与既有派发测试一样用 SQL 模拟一次领取；
//! 衔接、尝试、起写与证据都走生产端口。传输是计数替身，不连接任何外部服务。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use auction_application::consignment::{ConsignmentFuture, DispatchService, PostPermit};
use auction_domain::consignment::dispatch::{AttemptKey, dispatch_round_id};
use auction_jobs::consignment::{
    ConsignmentDispatchRuntime, DispatchExecution, DispatchLease, DispatchRound, DispatchTransport,
    DispatchWork, DispatchWorkSource, PreparedDispatch, SentObservation,
};
use auction_storage::{DatabaseStore, RoundLease, RoundLeaseRequest};
use tokio::sync::Notify;
use uuid::Uuid;

use super::super::contracts::fixture::OBLIGATION_1;
use super::fixture::{PRICE, Seeded, at, binding, frozen, round_key, uuid};

/// 已领取的那一项寄售义务；衔接经生产的 `lease_dispatch_round`。
struct Claimed {
    store: DatabaseStore,
    work: DispatchWork,
}

impl DispatchWorkSource for Claimed {
    fn claim_next<'a>(
        &'a self,
        _worker_id: &'a str,
        _lease: Duration,
    ) -> ConsignmentFuture<'a, Option<DispatchWork>> {
        Box::pin(async { Ok(Some(self.work.clone())) })
    }

    fn lease_round<'a>(&'a self, work: &'a DispatchWork) -> ConsignmentFuture<'a, DispatchLease> {
        Box::pin(async move {
            let request = RoundLeaseRequest {
                obligation_id: work.obligation_id,
                claim_token: work.claim_token,
                claim_fencing: work.claim_fencing,
                round_id: dispatch_round_id(&work.key),
            };
            Ok(match self.store.lease_dispatch_round(&request).await? {
                RoundLease::Leased { delivery_id, claim_token, fence } => DispatchLease::Leased {
                    attempt: AttemptKey { delivery_id, fence },
                    claim_token,
                },
                RoundLease::Busy => DispatchLease::Busy,
                RoundLease::Settled => DispatchLease::Settled,
                RoundLease::Rejected => DispatchLease::Rejected,
            })
        })
    }
}

/// 计数替身：每次执行记一次远端 POST，并给出 Sent 证明。
#[derive(Default)]
struct Remote {
    posts: AtomicUsize,
}

impl DispatchTransport for Remote {
    fn prepare<'a>(
        &'a self,
        _work: &'a DispatchWork,
        _attempt: AttemptKey,
        _claim_token: Uuid,
    ) -> ConsignmentFuture<'a, Option<PreparedDispatch>> {
        Box::pin(async {
            Ok(Some(PreparedDispatch {
                credential: binding(),
                cash_evidence: None,
                handle: Box::new(()),
            }))
        })
    }

    fn execute(
        &self,
        _prepared: PreparedDispatch,
        _permit: PostPermit,
    ) -> ConsignmentFuture<'_, DispatchExecution> {
        self.posts.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(DispatchExecution::Proven(SentObservation {
                sent_mid: "9000001".to_owned(),
                observed_at: at("2026-01-01T05:00:00Z"),
                validator_version: "fake-validator".to_owned(),
                evidence_digest: [7; 32],
            }))
        })
    }
}

fn executor(
    seeded: &Seeded,
    work: &DispatchWork,
    remote: &Arc<Remote>,
) -> ConsignmentDispatchRuntime {
    ConsignmentDispatchRuntime {
        work: Arc::new(Claimed { store: seeded.runtime.clone(), work: work.clone() }),
        service: Arc::new(DispatchService::new(Arc::new(seeded.runtime.clone()))),
        transport: remote.clone(),
        worker_id: "fake-worker".to_owned(),
        lease_duration: Duration::from_secs(300),
        wakeup: Arc::new(Notify::new()),
        paused: Arc::new(|| false),
    }
}

async fn facts(seeded: &Seeded) -> (i64, i64, String) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1),
                (SELECT count(*) FROM public.consignment_dispatch_evidence),
                (SELECT status FROM public.deliveries WHERE id = $1)",
    )
    .bind(seeded.delivery_id)
    .fetch_one(seeded.pool())
    .await
    .expect("读取起写与证据")
}

#[tokio::test]
async fn postgres_consignment_dispatch_two_executors_write_start_one_round_once() {
    let seeded = Seeded::purchased().await;
    let claim_token = Uuid::new_v4();
    let claim_fencing: i64 = sqlx::query_scalar(
        "UPDATE public.settlement_obligation_fulfillments
         SET claim_token = $2, claimed_at = clock_timestamp(),
             claim_expires_at = clock_timestamp() + interval '5 minutes',
             claim_fencing = claim_fencing + 1, state_version = state_version + 1
         WHERE obligation_id = $1 RETURNING claim_fencing",
    )
    .bind(uuid(OBLIGATION_1))
    .bind(claim_token)
    .fetch_one(seeded.pool())
    .await
    .expect("模拟寄售义务领取");
    let work = DispatchWork {
        obligation_id: uuid(OBLIGATION_1),
        claim_token,
        claim_fencing,
        key: round_key(),
        frozen: frozen(PRICE),
    };
    let remote = Arc::new(Remote::default());
    let first = executor(&seeded, &work, &remote);
    let second = executor(&seeded, &work, &remote);

    let (a, b) = tokio::join!(first.run_once(), second.run_once());
    let mut outcomes = [a.expect("executor 一"), b.expect("executor 二")];
    outcomes.sort_by_key(|outcome| *outcome == DispatchRound::Sent);
    assert_eq!(outcomes[1], DispatchRound::Sent, "{outcomes:?}");
    assert!(matches!(outcomes[0], DispatchRound::Deferred | DispatchRound::Closed), "{outcomes:?}");
    assert_eq!(remote.posts.load(Ordering::SeqCst), 1);
    assert_eq!(facts(&seeded).await, (1, 1, "succeeded".to_owned()));

    // 之后任何 executor 再处理同一项都只得到“已终结”，零第二次 POST。
    assert_eq!(second.run_once().await.expect("再处理"), DispatchRound::Closed);
    assert_eq!(remote.posts.load(Ordering::SeqCst), 1);
    assert_eq!(facts(&seeded).await, (1, 1, "succeeded".to_owned()));
}
