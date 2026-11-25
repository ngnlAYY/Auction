//! 订单复核消费端（PIWW3 提交 B）端到端：jobs 复核运行时经生产的领取、既有证据写口与完成，按原 attempt
//! 只读查 Sent——查到即补记 Sent 绑定、结果为 `bound`；查不到结果为 `still_unknown`。两种情形都不新增任何
//! 起写、派发尝试或 Take 起写（复核绝不 POST、不 Take）。读取端是假实现，不连接任何外部服务。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentFuture, DispatchService, OrderRecheckOutcome, RecheckDispatchAttempt,
    RecheckOrderInput,
};
use auction_application::core::Clock;
use auction_jobs::consignment::{
    DispatchSentReader, OrderRecheckRuntime, RecheckRound, SentObservation,
};
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use sqlx::PgPool;
use tokio::sync::Notify;
use uuid::Uuid;

use super::super::contracts::fixture::CONTRACT;
use super::super::order_admin::{ADMIN, command, service};
use super::fixture::{Seeded, at};

/// 假读取端：只计读取次数，按配置给出“查到”或“查不到”。
struct Reader {
    found: bool,
    reads: AtomicUsize,
}

impl DispatchSentReader for Reader {
    fn find_sent<'a>(
        &'a self,
        _attempt: &'a RecheckDispatchAttempt,
    ) -> ConsignmentFuture<'a, Option<SentObservation>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let found = self.found;
        Box::pin(async move {
            Ok(found.then(|| SentObservation {
                sent_mid: "9100001".to_owned(),
                observed_at: at("2026-01-01T06:00:00Z"),
                validator_version: "fake-validator".to_owned(),
                evidence_digest: [0x61; 32],
            }))
        })
    }
}

struct Now;

impl Clock for Now {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

fn runtime(store: &DatabaseStore, reader: &Arc<Reader>) -> OrderRecheckRuntime {
    OrderRecheckRuntime {
        store: Arc::new(store.clone()),
        reader: reader.clone(),
        service: Arc::new(DispatchService::new(Arc::new(store.clone()))),
        clock: Arc::new(Now),
        worker_id: "recheck-worker".to_owned(),
        lease_duration: Duration::from_secs(300),
        wakeup: Arc::new(Notify::new()),
        paused: Arc::new(|| false),
    }
}

/// Admin 排入一条复核工作（运行账号）。
async fn enqueue(store: &DatabaseStore, pool: &PgPool) {
    let head: Option<i64> = sqlx::query_scalar(
        "SELECT next_sequence - 1 FROM public.realtime_streams WHERE stream_key = $1",
    )
    .bind(format!("consignment_order:{CONTRACT}"))
    .fetch_optional(pool)
    .await
    .expect("读取事件流");
    service(store.clone())
        .recheck(RecheckOrderInput {
            admin_actor: ADMIN.to_owned(),
            command_id: command(0x5501),
            contract_id: Uuid::parse_str(CONTRACT).expect("合同"),
            expected_version: head.map_or(1, |head| head.max(1)).cast_unsigned(),
            request_id: None,
        })
        .await
        .expect("排入复核");
}

/// 外写相关事实：起写、派发尝试、Take 起写与 Take attempt 的行数。
async fn write_facts(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.delivery_send_attempts),
                (SELECT count(*) FROM public.consignment_dispatch_attempts),
                (SELECT count(*) FROM public.mail_disposition_write_starts),
                (SELECT count(*) FROM public.mail_disposition_attempts)",
    )
    .fetch_one(pool)
    .await
    .expect("读取外写事实")
}

/// （投递状态，Sent 绑定证据数）。
async fn delivery_state(pool: &PgPool, delivery_id: Uuid) -> (String, i64) {
    sqlx::query_as(
        "SELECT (SELECT status FROM public.deliveries WHERE id = $1),
                (SELECT count(*) FROM public.consignment_dispatch_evidence
                 WHERE category = 'sent_binding')",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("读取投递与证据")
}

#[tokio::test]
async fn postgres_consignment_order_recheck_consumer_binds_the_found_sent_without_writing() {
    let (seeded, _started) = Seeded::first_dispatch_started().await;
    let pool = seeded.pool();
    enqueue(&seeded.runtime, pool).await;
    let before = write_facts(pool).await;
    let reader = Arc::new(Reader { found: true, reads: AtomicUsize::new(0) });

    let round = runtime(&seeded.runtime, &reader).run_once().await.expect("复核");

    assert_eq!(round, RecheckRound::Completed(OrderRecheckOutcome::Bound));
    assert_eq!(reader.reads.load(Ordering::SeqCst), 1, "按原 attempt 只读一次");
    assert_eq!(delivery_state(pool, seeded.delivery_id).await, ("succeeded".to_owned(), 1));
    assert_eq!(write_facts(pool).await, before, "复核不得新增起写、尝试或 Take");
}

#[tokio::test]
async fn postgres_consignment_order_recheck_consumer_leaves_an_unfound_send_still_unknown() {
    let (seeded, _started) = Seeded::first_dispatch_started().await;
    let pool = seeded.pool();
    enqueue(&seeded.runtime, pool).await;
    let before = write_facts(pool).await;
    let reader = Arc::new(Reader { found: false, reads: AtomicUsize::new(0) });

    let round = runtime(&seeded.runtime, &reader).run_once().await.expect("复核");

    assert_eq!(round, RecheckRound::Completed(OrderRecheckOutcome::StillUnknown));
    assert_eq!(reader.reads.load(Ordering::SeqCst), 1);
    assert_eq!(delivery_state(pool, seeded.delivery_id).await, ("sending".to_owned(), 0));
    assert_eq!(write_facts(pool).await, before, "复核不得新增起写、尝试或 Take");
}
