//! 补寄新轮的来源：已排队的补寄处置列入待办；新轮投递在存储内渲染、只备好一次（并发只建一封），
//! 冻结权利人是义务受益人（合同冻结买方），D0 = 备好时的数据库时刻 + 合同冻结的付款期限；备好的
//! 投递交给授权建轮，同事务把履约行放回可领取，补寄轮经派发领取、衔接与起写走通。

use std::sync::Arc;

use auction_application::consignment::{
    DispatchService, DispatchWorkStore, OrderResolveAction, OrderResolveReceipt,
    RedispatchPrepared, WriteStart,
};
use auction_domain::consignment::dispatch::{AttemptKey, RoundOrigin};
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::CONTRACT;
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::PAY_DELIVERY;
use super::super::order_admin::{resolve_input, service as order_service};
use super::super::returns::fixture::{
    Rts, SIGNED_VALIDATOR, consume_input, seed_original_started, service as return_service,
};
use super::fixture::{CONTENT, FakeContent, binding, prepare_send_credential, uuid, work_store};
use crate::support::TestDatabase;

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

async fn deliveries_of(pool: &PgPool, obligation: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.deliveries WHERE origin_id = $1::text")
        .bind(obligation)
        .fetch_one(pool)
        .await
        .expect("投递数")
}

#[tokio::test]
async fn postgres_consignment_redispatch_prepares_the_new_round_delivery_once_for_the_buyer() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let runtime = Arc::new(database.store().await.expect("以运行账号连接"));
    let source = work_store(&runtime, CONTENT);
    // 上一轮原件 RTS 已接管并精确消费，Admin 已排队补寄。
    let rts = Rts { n: 1 };
    let (receipt, claim) = seed_original_started(pool, &runtime, rts).await;
    return_service(&runtime)
        .consume(consume_input(&receipt, rts, claim, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("消费封闭上一轮");
    // 上一轮经真实衔接后履约行指向它的投递（enqueued）；补寄建轮须在同一事务把它放回可领取。
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

    let pending = source.queued_redispatches(10).await.expect("补寄待办");
    assert_eq!(pending.len(), 1, "{pending:?}");
    let item = &pending[0];
    assert_eq!((item.dispatch_id, item.key.round, item.prepared.as_ref()), (dispatch_id, 2, None));
    assert_eq!(item.closed_by_return_intake, receipt.intake_id);
    let obligation = item.key.obligation_id;
    let before = deliveries_of(pool, obligation).await;

    // 反例：渲染出的附件不是合同规范装备 → 失败关闭，不建投递。
    let wrong = work_store(&runtime, FakeContent { body: 0xe2, wrong_equipment: true });
    assert_eq!(wrong.prepare_redispatch(dispatch_id).await.expect("备好"), None);
    assert_eq!(deliveries_of(pool, obligation).await, before);

    // 正例：两个备好者并发，只建一封投递，另一个得到同一投递与同一冻结身份的重放。
    let (first, second) = tokio::join!(
        source.prepare_redispatch(dispatch_id),
        source.prepare_redispatch(dispatch_id)
    );
    let mut outcomes = [first.expect("备好一"), second.expect("备好二")];
    outcomes.sort_by_key(|outcome| outcome.as_ref().map(|prepared| prepared.replayed));
    let [
        Some(RedispatchPrepared { delivery_id, frozen, replayed: false }),
        Some(RedispatchPrepared { delivery_id: again, frozen: again_frozen, replayed: true }),
    ] = outcomes
    else {
        panic!("恰一次新建、一次重放：{outcomes:?}");
    };
    assert_eq!((delivery_id, &frozen), (again, &again_frozen));
    assert_eq!(deliveries_of(pool, obligation).await, before + 1);
    // 冻结权利人是合同冻结买方；身份摘要与载荷一致；D0 = 备好时的数据库时刻 + 冻结付款期限。
    assert_eq!(frozen.rights_holder_identity, "FakeSellerTwo");
    let stored: (String, String, String, Timestamp) = sqlx::query_as(
        "SELECT delivery.status, delivery.recipient, delivery.payload ->> 'identity_digest',
                delivery.created_at + make_interval(days => sale.payment_deadline_days::int)
         FROM public.deliveries AS delivery
         JOIN public.consignment_sales AS sale ON sale.contract_id = $2
         WHERE delivery.id = $1",
    )
    .bind(delivery_id)
    .bind(uuid(CONTRACT))
    .fetch_one(pool)
    .await
    .expect("新轮投递");
    let digest: String =
        frozen.identity_digest().iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        (stored.0.as_str(), stored.1.as_str(), stored.2.as_str()),
        ("prepared", "FakeSellerTwo", digest.as_str())
    );
    assert_eq!(frozen.d0, stored.3, "新轮 D0 = 备好时刻 + 合同冻结的付款期限");

    // 反例：已备好后内容来源改渲染另一份正文，再备好只重放原投递与原冻结身份，载荷摘要不变。
    let rerender = work_store(&runtime, FakeContent { body: 0xe3, wrong_equipment: false });
    let replay = rerender.prepare_redispatch(dispatch_id).await.expect("备好");
    assert_eq!(
        replay,
        Some(RedispatchPrepared { delivery_id, frozen: frozen.clone(), replayed: true })
    );
    assert_eq!(deliveries_of(pool, obligation).await, before + 1);
    let digest_after: String = sqlx::query_scalar(
        "SELECT payload ->> 'identity_digest' FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("载荷摘要");
    assert_eq!(digest_after, digest);

    // 待办给出已备好的投递与原样冻结身份，交给授权即建第 2 轮；之后不再列出。
    let pending = source.queued_redispatches(10).await.expect("补寄待办");
    let prepared = pending[0].prepared.clone().expect("已备好");
    assert_eq!((prepared.delivery_id, &prepared.frozen), (delivery_id, &frozen));
    let origin = RoundOrigin::Redispatch { closed_by_return_intake: receipt.intake_id };
    let dispatch = DispatchService::new(runtime.clone());
    let round = dispatch
        .authorize(pending[0].key, origin, prepared.delivery_id, prepared.frozen)
        .await
        .expect("授权新轮");
    assert_eq!(
        (round.round_id, round.delivery_id, round.replayed),
        (dispatch_id, delivery_id, false)
    );
    assert!(source.queued_redispatches(10).await.expect("补寄待办").is_empty());
    let reset: (String, Option<Uuid>) = sqlx::query_as(
        "SELECT execution_state, delivery_id FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1",
    )
    .bind(obligation)
    .fetch_one(pool)
    .await
    .expect("履约行");
    assert_eq!(reset, ("pending".to_owned(), None), "建轮同事务放回可领取");

    // 补寄轮经派发领取、衔接改指新轮投递，起写恰一次。
    prepare_send_credential(pool).await;
    owner.test_register_contract_rights(uuid(CONTRACT)).await.expect("补登合同权利");
    let now = Timestamp::now();
    let work = source
        .claim_dispatch_work(now, now + Duration::minutes(5))
        .await
        .expect("领取")
        .expect("补寄轮可领取");
    assert_eq!((work.key, &work.frozen), (pending[0].key, &frozen));
    let (leased, token, fence) = runtime
        .test_lease_dispatch_round(
            work.obligation_id,
            work.claim_token,
            work.claim_fencing,
            dispatch_id,
        )
        .await
        .expect("衔接")
        .expect("取得新轮投递租约");
    assert_eq!(leased, delivery_id);
    let attempt = AttemptKey { delivery_id, fence };
    dispatch.start_attempt(work.key, attempt, token, frozen.clone()).await.expect("登记尝试");
    let started = dispatch.mark_write_started(attempt, token, Some(binding())).await.expect("起写");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "{started:?}");
    let send_attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("起写数");
    assert_eq!(send_attempts, 1);
}
