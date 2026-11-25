//! 派发与补寄的 Worker 接线端到端（PIWW3）：jobs executor 经生产的寄售义务领取、轮衔接、尝试、起写与
//! Sent 证据，把首轮投递推进到 `succeeded`，恰一次 POST；补寄消费端按处置备好并授权新轮后，同一
//! executor 再把新轮走到 `succeeded`。
//!
//! 工作来源适配与 Worker 装配根（`apps/auction-worker/src/bootstrap/mail/consignment/work.rs`）同一委托
//! 写法；测试不能依赖装配根 crate，所以在此按同一映射写一份。传输是计数替身，不连接任何外部服务。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DispatchService, DispatchWorkStore, OrderResolveAction,
    OrderResolveReceipt, PostPermit,
};
use auction_domain::consignment::dispatch::{AttemptKey, dispatch_round_id};
use auction_jobs::consignment::{
    ConsignmentDispatchRuntime, DispatchExecution, DispatchLease, DispatchRound, DispatchTransport,
    DispatchWork, DispatchWorkSource, PendingRedispatch, PreparedDispatch, RedispatchRuntime,
    RedispatchWorkSource, SentObservation, run_redispatch,
};
use auction_storage::{DatabaseStore, DispatchWorkRepository, RoundLease, RoundLeaseRequest};
use auction_time::{Duration as TimeDelta, Timestamp};
use sqlx::PgPool;
use tokio::sync::{Notify, watch};
use uuid::Uuid;

use super::super::contracts::fixture::CONTRACT;
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::PAY_DELIVERY;
use super::super::order_admin::{resolve_input, service as order_service};
use super::super::returns::fixture::{
    Rts, SIGNED_VALIDATOR, consume_input, seed_original_started, service as return_service,
};
use super::fixture::{CONTENT, Seeded, at, binding, prepare_send_credential, uuid, work_store};
use crate::support::TestDatabase;

/// 与 Worker 装配根同一委托：领取经派发工作端口，衔接经存储的轮租约。
pub(in crate::consignment) struct Chain {
    pub(in crate::consignment) store: DatabaseStore,
    pub(in crate::consignment) work: DispatchWorkRepository,
}

impl DispatchWorkSource for Chain {
    fn claim_next<'a>(
        &'a self,
        _worker_id: &'a str,
        lease: Duration,
    ) -> ConsignmentFuture<'a, Option<DispatchWork>> {
        Box::pin(async move {
            let now = Timestamp::now();
            let lease = TimeDelta::from_std(lease).ok().ok_or(ConsignmentError::Unavailable)?;
            let claim = self.work.claim_dispatch_work(now, now + lease).await?;
            Ok(claim.map(|claim| DispatchWork {
                obligation_id: claim.obligation_id,
                claim_token: claim.claim_token,
                claim_fencing: claim.claim_fencing,
                key: claim.key,
                frozen: claim.frozen,
            }))
        })
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

/// 与 Worker 装配根同一委托：未备好的补寄按处置的轮 ID 交存储备好。
struct Queued(DispatchWorkRepository);

impl RedispatchWorkSource for Queued {
    fn pending_redispatches(&self, limit: u32) -> ConsignmentFuture<'_, Vec<PendingRedispatch>> {
        Box::pin(async move {
            let mut pending = Vec::new();
            for item in self.0.queued_redispatches(limit).await? {
                let prepared = match item.prepared {
                    Some(prepared) => (prepared.delivery_id, prepared.frozen),
                    None => match self.0.prepare_redispatch(item.dispatch_id).await? {
                        Some(prepared) => (prepared.delivery_id, prepared.frozen),
                        None => continue,
                    },
                };
                pending.push(PendingRedispatch {
                    dispatch_id: item.dispatch_id,
                    key: item.key,
                    closed_by_return_intake: item.closed_by_return_intake,
                    delivery_id: prepared.0,
                    frozen: prepared.1,
                });
            }
            Ok(pending)
        })
    }
}

/// 计数替身：每次执行记一次远端 POST，并给出 Sent 证明（每封不同的 MID）。
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
        let post = self.posts.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            Ok(DispatchExecution::Proven(SentObservation {
                sent_mid: format!("900000{post}"),
                observed_at: at("2026-01-01T05:00:00Z"),
                validator_version: "fake-validator".to_owned(),
                evidence_digest: [7; 32],
            }))
        })
    }
}

fn executor(
    runtime: &DatabaseStore,
    remote: &Arc<Remote>,
    service: &Arc<DispatchService>,
) -> ConsignmentDispatchRuntime {
    ConsignmentDispatchRuntime {
        work: Arc::new(Chain { store: runtime.clone(), work: work_store(runtime, CONTENT) }),
        service: service.clone(),
        transport: remote.clone(),
        worker_id: "fake-worker".to_owned(),
        lease_duration: Duration::from_secs(300),
        wakeup: Arc::new(Notify::new()),
        paused: Arc::new(|| false),
    }
}

/// 某投递的（状态、起写数、Sent 证据数）。
pub(in crate::consignment) async fn delivery_facts(
    pool: &PgPool,
    delivery_id: Uuid,
) -> (String, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT status FROM public.deliveries WHERE id = $1),
                (SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1),
                (SELECT count(*) FROM public.consignment_dispatch_evidence AS evidence
                 JOIN public.consignment_dispatch_attempts AS attempt
                   ON attempt.attempt_id = evidence.attempt_id
                 WHERE attempt.delivery_id = $1 AND evidence.category = 'sent_binding')",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("读取投递、起写与证据")
}

#[tokio::test]
async fn postgres_consignment_dispatch_executor_drives_the_first_round_to_succeeded_once() {
    let seeded = Seeded::purchased().await;
    let remote = Arc::new(Remote::default());
    let service = Arc::new(DispatchService::new(Arc::new(seeded.runtime.clone())));
    let executor = executor(&seeded.runtime, &remote, &service);

    // 生产领取 → 衔接 → 起写 → 唯一一次 POST → Sent 证据，同事务推进到 succeeded。
    assert_eq!(executor.run_once().await.expect("首轮"), DispatchRound::Sent);
    assert_eq!(remote.posts.load(Ordering::SeqCst), 1);
    assert_eq!(
        delivery_facts(seeded.pool(), seeded.delivery_id).await,
        ("succeeded".to_owned(), 1, 1)
    );

    // 反例：再跑一轮不再领到已寄出的轮，零第二次 POST，事实不变。
    assert_eq!(executor.run_once().await.expect("再跑"), DispatchRound::Idle);
    assert_eq!(remote.posts.load(Ordering::SeqCst), 1);
    assert_eq!(
        delivery_facts(seeded.pool(), seeded.delivery_id).await,
        ("succeeded".to_owned(), 1, 1)
    );
}

/// 订单事件流的当前版本（resolve 的预期版本）。
async fn order_version(pool: &PgPool) -> u64 {
    let head: Option<i64> = sqlx::query_scalar(
        "SELECT next_sequence - 1 FROM public.realtime_streams WHERE stream_key = $1",
    )
    .bind(format!("consignment_order:{CONTRACT}"))
    .fetch_optional(pool)
    .await
    .expect("读取事件流");
    head.map_or(1, |head| head.max(1)).cast_unsigned()
}

#[tokio::test]
async fn postgres_consignment_redispatch_consumer_and_executor_send_the_new_round_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let runtime = database.store().await.expect("以运行账号连接");
    // 上一轮原件 RTS 已接管并精确消费，上一轮履约行停在它的投递上，Admin 已排队补寄。
    let rts = Rts { n: 1 };
    let shared = Arc::new(runtime.clone());
    let (receipt, claim) = seed_original_started(pool, &shared, rts).await;
    return_service(&shared)
        .consume(consume_input(&receipt, rts, claim, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("消费封闭上一轮");
    succeeds(
        pool,
        &format!(
            "UPDATE public.settlement_obligation_fulfillments AS fulfillment
             SET execution_state = 'enqueued', delivery_id = delivery.id,
                 prepare_command_id = delivery.command_id,
                 prepare_request_digest = delivery.request_digest,
                 activate_command_id = gen_random_uuid(),
                 activate_request_digest = delivery.request_digest,
                 state_version = fulfillment.state_version + 2
             FROM public.deliveries AS delivery
             WHERE delivery.id = '{PAY_DELIVERY}'
               AND fulfillment.obligation_id = delivery.origin_id::uuid;"
        ),
    )
    .await;
    let queued = order_service(database.store().await.expect("运行账号"))
        .resolve(resolve_input(
            1,
            uuid(CONTRACT),
            order_version(pool).await,
            OrderResolveAction::RedispatchOriginal,
            receipt.intake_id,
        ))
        .await
        .expect("排队补寄");
    let OrderResolveReceipt::RedispatchQueued { dispatch_id, .. } = queued else {
        panic!("应为 redispatch_queued：{queued:?}");
    };
    prepare_send_credential(pool).await;
    owner.test_register_contract_rights(uuid(CONTRACT)).await.expect("补登合同权利");

    // 补寄消费端：备好新轮投递并授权建轮；之后待办清空。
    let service = Arc::new(DispatchService::new(shared.clone()));
    let consumer = Arc::new(RedispatchRuntime {
        source: Arc::new(Queued(work_store(&runtime, CONTENT))),
        service: service.clone(),
        wakeup: Arc::new(Notify::new()),
    });
    let (drain, drained) = watch::channel(false);
    let loop_task = tokio::spawn(run_redispatch(consumer, drained, |_, _| {}));
    let source = work_store(&runtime, CONTENT);
    let mut authorized = false;
    for _ in 0..100 {
        if source.queued_redispatches(10).await.expect("补寄待办").is_empty() {
            authorized = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drain.send(true).expect("排空补寄循环");
    loop_task.await.expect("补寄循环结束");
    assert!(authorized, "补寄消费端应把已排队的补寄建成新轮");
    let new_delivery: Uuid = sqlx::query_scalar(
        "SELECT delivery_id FROM public.consignment_dispatch_rounds WHERE round_id = $1",
    )
    .bind(dispatch_id)
    .fetch_one(pool)
    .await
    .expect("新轮投递");

    // 派发 executor 把新轮走到 succeeded，恰一次 POST。
    let remote = Arc::new(Remote::default());
    let executor = executor(&runtime, &remote, &service);
    assert_eq!(executor.run_once().await.expect("补寄轮"), DispatchRound::Sent);
    assert_eq!(remote.posts.load(Ordering::SeqCst), 1);
    assert_eq!(delivery_facts(pool, new_delivery).await, ("succeeded".to_owned(), 1, 1));
    assert_eq!(executor.run_once().await.expect("再跑"), DispatchRound::Idle);
    assert_eq!(remote.posts.load(Ordering::SeqCst), 1);
}
