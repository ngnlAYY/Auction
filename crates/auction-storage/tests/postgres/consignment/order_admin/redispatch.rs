//! 补寄原件与 recheck：补寄只排队新轮（不自动开轮、不发 POST），上一轮必须已由退件接管与消费明确封闭；
//! recheck 只追加持久复核工作，不改任何订单状态。

use std::sync::Arc;

use auction_application::consignment::{
    DepositBusinessRejection, OrderResolveAction, OrderResolveReceipt, RecheckOrderInput,
};
use auction_domain::consignment::dispatch::{DispatchRoundKey, dispatch_round_id};
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1};
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{PRICE, PURCHASED_AT, seed_purchased};
use super::super::returns::fixture::{
    Rts, SIGNED_VALIDATOR, consume_input, seed_original_started, service as return_service,
};
use super::effects::current_version;
use super::{ADMIN, command, count, ledger_of, rejection, resolve_input, service};
use crate::support::TestDatabase;

fn contract() -> Uuid {
    Uuid::parse_str(CONTRACT).expect("合同")
}

/// 反例：上一轮的原件退件接管还没有消费（未明确封闭）→ 证据不足，不排队。正例：消费之后只排队轮 2，
/// 不开轮（轮数不变）；同命令重放原回执；另一条命令再排同一轮被拒（同一轮至多排队一次）。
#[tokio::test]
async fn postgres_consignment_order_redispatch_only_queues_a_new_round_after_the_previous_one_is_closed()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let returns = Arc::new(database.store().await.expect("以运行账号连接"));
    let rts = Rts { n: 1 };
    let (receipt, claim) = seed_original_started(pool, &returns, rts).await;
    let intake = receipt.intake_id;
    let service = service(database.store().await.expect("运行账号"));
    let rounds = || count(pool, "SELECT count(*) FROM public.consignment_dispatch_rounds");
    assert_eq!(rounds().await, 1);

    let early = service
        .resolve(resolve_input(
            1,
            contract(),
            current_version(pool).await,
            OrderResolveAction::RedispatchOriginal,
            intake,
        ))
        .await
        .expect_err("上一轮未封闭");
    assert_eq!(rejection(&early), Some(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_order_resolutions").await, 0);

    return_service(&returns)
        .consume(consume_input(&receipt, rts, claim, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("消费封闭上一轮");
    // 接管与消费都追加订单阶段事件，订单版本随之前进：按当前版本提交（重放沿用同一请求）。
    let version = current_version(pool).await;

    let queued = service
        .resolve(resolve_input(
            2,
            contract(),
            version,
            OrderResolveAction::RedispatchOriginal,
            intake,
        ))
        .await
        .expect("排队补寄");
    let OrderResolveReceipt::RedispatchQueued { dispatch_id, .. } = queued else {
        panic!("应为 redispatch_queued：{queued:?}");
    };
    let next_round = DispatchRoundKey {
        contract_id: contract(),
        obligation_id: Uuid::parse_str(OBLIGATION_1).expect("义务"),
        round: 2,
    };
    assert_eq!(dispatch_id, dispatch_round_id(&next_round), "补寄只指向紧接的下一轮");
    assert_eq!(rounds().await, 1, "排队不开轮、不发任何 POST");
    assert_eq!(ledger_of(pool, command(2)).await, Some((202, "ok".to_owned())));

    let replay = service
        .resolve(resolve_input(
            2,
            contract(),
            version,
            OrderResolveAction::RedispatchOriginal,
            intake,
        ))
        .await
        .expect("重放");
    assert_eq!(replay, queued);
    let twice = service
        .resolve(resolve_input(
            3,
            contract(),
            current_version(pool).await,
            OrderResolveAction::RedispatchOriginal,
            intake,
        ))
        .await
        .expect_err("同一轮已排队");
    assert_eq!(rejection(&twice), Some(DepositBusinessRejection::DispositionConflict));
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_order_resolutions").await, 1);
}

/// 反例：上一轮已由消费封闭，但此后又出现了更新的轮（补寄已发生或被别的路径开过）→ 确定拒绝，不再排队：
/// 处置为零，轮数不变。去掉“只能紧接被封闭的那一轮”的判断，补寄会指向一个已存在的轮。
#[tokio::test]
async fn postgres_consignment_order_redispatch_refuses_when_a_newer_round_already_exists() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let returns = Arc::new(database.store().await.expect("以运行账号连接"));
    let rts = Rts { n: 1 };
    let (receipt, claim) = seed_original_started(pool, &returns, rts).await;
    return_service(&returns)
        .consume(consume_input(&receipt, rts, claim, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("消费封闭上一轮");
    let consumption: Uuid =
        sqlx::query_scalar("SELECT id FROM public.consignment_return_consumptions")
            .fetch_one(pool)
            .await
            .expect("消费");
    // 其他路径已经为同一义务开了第 2 轮（引用同一接管与消费）。
    let (delivery, round) =
        ("7e800000-0000-4000-8000-000000000001", "7e810000-0000-4000-8000-000000000001");
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('{delivery}', '{delivery}', decode(repeat('c5', 32), 'hex'), 'settlement',
                     '{OBLIGATION_1}', 'FakeSellerTwo', '{{}}'::jsonb, 'prepared', 0, 0,
                     '{PURCHASED_AT}', '{PURCHASED_AT}');
             INSERT INTO public.consignment_dispatch_rounds
                (contract_id, obligation_id, round, round_id, delivery_id, rights_holder_identity,
                 bot_uid, world_id, subject, template, body_digest, round_ref, d0, credits, cod,
                 prior_round, authorized_by_intake, authorized_by_consumption)
             VALUES ('{CONTRACT}', '{OBLIGATION_1}', 2, '{round}', '{delivery}', 'FakeSellerTwo',
                     '12345', 'main', 'fake subject', 'consignment_cod',
                     decode(repeat('d1', 32), 'hex'), '{round}', '{PURCHASED_AT}', 0, {PRICE},
                     1, '{intake}', '{consumption}');",
            intake = receipt.intake_id,
        ),
    )
    .await;
    let rounds = || count(pool, "SELECT count(*) FROM public.consignment_dispatch_rounds");
    let before = rounds().await;
    let service = service(database.store().await.expect("运行账号"));

    let error = service
        .resolve(resolve_input(
            1,
            contract(),
            current_version(pool).await,
            OrderResolveAction::RedispatchOriginal,
            receipt.intake_id,
        ))
        .await
        .expect_err("已有更新的轮");

    assert_eq!(rejection(&error), Some(DepositBusinessRejection::ActionNotAllowed));
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_order_resolutions").await, 0);
    assert_eq!(rounds().await, before, "拒绝不开轮");
    assert_eq!(ledger_of(pool, command(1)).await, Some((403, "action_not_allowed".to_owned())));
}

/// recheck 只追加一条持久复核工作：同命令重放返回同一工作 ID 且不重复排队；版本不符确定拒绝；
/// 订单阶段事件流不被触碰（不改任何状态）。
#[tokio::test]
async fn postgres_consignment_order_recheck_queues_one_durable_work_item_without_changing_state() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_purchased(pool).await;
    let service = service(database.store().await.expect("运行账号"));
    let input = |n: u128, version: u64| RecheckOrderInput {
        admin_actor: ADMIN.to_owned(),
        command_id: command(n),
        contract_id: contract(),
        expected_version: version,
        request_id: None,
    };

    let first = service.recheck(input(1, 1)).await.expect("排入复核");
    assert_eq!(first.contract_version, 1);
    let again = service.recheck(input(1, 1)).await.expect("重放");
    assert_eq!(again, first);
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_order_rechecks").await, 1);

    let stale = service.recheck(input(2, 5)).await.expect_err("版本不符");
    assert_eq!(rejection(&stale), Some(DepositBusinessRejection::StaleVersion));
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_order_rechecks").await, 1);
    let events: i64 = count(
        pool,
        "SELECT count(*) FROM public.realtime_events WHERE stream_key LIKE 'consignment_order:%'",
    )
    .await;
    assert_eq!(events, 0, "recheck 不追加订单阶段事件");
}
