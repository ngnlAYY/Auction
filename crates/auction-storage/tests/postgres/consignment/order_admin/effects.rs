//! resolve 的资金与封闭类动作：相反付款在锁内复验（卖款已预留）、退款与订单阶段事件同一事务、
//! `cancel_unpaid` 经 7A-2 的封闭写端、`retain_entitlement` 经 6B 写端且零重复资金。
//!
//! 只测核心规则：资金（转移一次、守恒、零痕迹）、命令幂等（重放、异命令冲突）、原子性（同事务回滚）、失败关闭。

use std::sync::Arc;

use auction_application::consignment::{
    AutoRelistSetting, ConsignmentError, DepositBusinessRejection, OrderResolveAction,
    OrderResolveReceipt, ReturnDispositionService, ReturnDispositionStore,
};
use auction_application::settlement::{CashLeg, CashPhase, SettleCash};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_2};
use super::super::deposit::support::succeeds;
use super::super::funding::fixture::{UID, entries_of, id, mutation};
use super::super::ledger136::fixture::{round_sql, send_sql};
use super::super::ledger136::returns::seed as seed_unpaid_sale;
use super::super::recovery::sale_recovery::refund_call::{
    PAYOUT_ATTEMPT, PAYOUT_DELIVERY, PAYOUT_MID, PAYOUT_ROUND, allocation, closures,
};
use super::super::recovery::unpaid_recovery::{DISPOSITION, disposition_sql, seed_consumption};
use super::super::returns::fixture::{Rts, seed_consumed};
use super::resolve::{
    contract, funds, order_events, paid_sale_with_loss_proof, proof, resolutions,
};
use super::{command, count, ledger_of, rejection, resolve_input, service};
use crate::support::TestDatabase;

/// 订单阶段事件流的头部序号（没有事件时为 1），即 resolve 的预期版本。
pub(super) async fn current_version(pool: &PgPool) -> u64 {
    let head: Option<i64> = sqlx::query_scalar(
        "SELECT next_sequence - 1 FROM public.realtime_streams WHERE stream_key = $1",
    )
    .bind(format!("consignment_order:{CONTRACT}"))
    .fetch_optional(pool)
    .await
    .expect("读取事件流");
    head.map_or(1, |head| head.max(1)).cast_unsigned()
}

/// 卖款腿的真实预留：派发尝试、轮（Credits 为卖款 S）与 `settle_cash(reserve)` 把卖款分配置为 reserved。
async fn reserve_the_seller_payment(database: &TestDatabase, pool: &PgPool) {
    let round =
        round_sql(PAYOUT_ROUND, PAYOUT_DELIVERY, OBLIGATION_2, PAYOUT_ATTEMPT, "cash_payout");
    let payout_round =
        round.replace("'2026-01-01T04:00:00Z', 0, 101,", "'2026-01-01T04:00:00Z', 96, 0,");
    assert_ne!(payout_round, round, "夹具应把轮的 Credits 改为卖款 96");
    succeeds(pool, &(send_sql(PAYOUT_DELIVERY, OBLIGATION_2, PAYOUT_MID, 0) + &payout_round)).await;
    database
        .store()
        .await
        .expect("运行账号")
        .test_settle_cash(SettleCash {
            mutation: mutation(31),
            contract_id: id(CONTRACT),
            obligation_id: id(OBLIGATION_2),
            delivery_id: id(PAYOUT_DELIVERY),
            leg: CashLeg::Seller,
            phase: CashPhase::Reserve,
            amount: 96,
        })
        .await
        .expect("预留卖款");
}

/// 反例：卖款已被预留（已进入派发流程）→ 写端在 UID 账户锁内复验得到 `opposite_payment_open`：不转换、零资金
/// 痕迹、不入封闭事实与处置、不追加阶段事件。去掉锁内复验，转换会因分配状态报冲突或直接转走已预留的卖款，
/// 本用例随之失败。
#[tokio::test]
async fn postgres_consignment_order_resolve_rechecks_the_reserved_seller_payment_inside_the_lock() {
    let (database, pool, runtime) = paid_sale_with_loss_proof().await;
    reserve_the_seller_payment(&database, &pool).await;
    let transfers = entries_of(&pool, UID, "transfer").await;
    let state: String = sqlx::query_scalar(
        "SELECT state FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = 'seller' AND consignment_contract_id = $2",
    )
    .bind(UID)
    .bind(id(CONTRACT))
    .fetch_one(&pool)
    .await
    .expect("卖款分配");
    assert_eq!(state, "reserved", "夹具应是真实的预留状态");
    // 夹具的 settle_cash 预留已同事务追加一条阶段事件；本用例只断言 resolve 不再追加。
    let events = order_events(&pool).await;

    let error = service(runtime)
        .resolve(resolve_input(1, contract(), 1, OrderResolveAction::RefundLost, proof()))
        .await
        .expect_err("卖款已预留");

    assert_eq!(rejection(&error), Some(DepositBusinessRejection::OppositePaymentOpen));
    assert_eq!(funds(&pool).await, (0, 96, 5), "卖款与手续费原样在账");
    assert_eq!(entries_of(&pool, UID, "transfer").await, transfers);
    assert_eq!((closures(&pool).await, resolutions(&pool).await), (0, 0));
    assert_eq!(order_events(&pool).await, events);
    assert_eq!(ledger_of(&pool, command(1)).await, Some((409, "opposite_payment_open".to_owned())));
}

/// 原子性：退款转换、封闭事实、处置记录、订单阶段事件与回执在同一个事务里。落账的最后一步失败（这里用一条
/// 恒假的 CHECK 模拟）时，资金转换随之整笔回滚：资金、封闭事实、事件与账本全部没有痕迹，也不是确定拒绝——
/// 调用方以原命令 ID 重试。退款与阶段事件分属两个事务时，资金会留在已转换状态，本用例随之失败。
#[tokio::test]
async fn postgres_consignment_order_resolve_refund_and_stage_event_commit_or_roll_back_together() {
    let (_database, pool, runtime) = paid_sale_with_loss_proof().await;
    succeeds(
        &pool,
        "ALTER TABLE public.consignment_order_resolutions
             ADD CONSTRAINT c7_test_fault CHECK (false);",
    )
    .await;
    let transfers = entries_of(&pool, UID, "transfer").await;

    let error = service(runtime)
        .resolve(resolve_input(1, contract(), 1, OrderResolveAction::RefundLost, proof()))
        .await
        .expect_err("落账失败");

    assert_eq!(rejection(&error), None, "失败不是确定拒绝：{error:?}");
    assert_eq!(funds(&pool).await, (0, 96, 5), "资金转换随同一事务回滚");
    assert_eq!(entries_of(&pool, UID, "transfer").await, transfers);
    assert_eq!((closures(&pool).await, resolutions(&pool).await), (0, 0));
    assert_eq!(order_events(&pool).await, 0);
    assert_eq!(ledger_of(&pool, command(1)).await, None, "没有占用命令 ID");
}

/// 正例：`cancel_unpaid` 经 7A-2 的写端写 Sale 封闭事实（依据是处置 ID），零资金分录；处置记录、订单阶段事件
/// 与回执同事务；同命令重放原回执；另一条命令再取消同一合同被拒。反例：证据不是该合同的处置。
#[tokio::test]
async fn postgres_consignment_order_resolve_cancel_unpaid_closes_the_sale_through_the_closure_writer()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let pool = owner.postgres_pool();
    seed_unpaid_sale(pool).await;
    seed_consumption(pool).await;
    succeeds(
        pool,
        &disposition_sql(
            DISPOSITION,
            ("close_at_limit", "held_unpaid_recovered", "manual_or_auto"),
            true,
        ),
    )
    .await;
    let disposition = Uuid::parse_str(DISPOSITION).expect("处置");
    let funding = "SELECT (SELECT count(*) FROM public.settlement_funding_entries)
                        + (SELECT count(*) FROM public.settlement_funding_allocations)
                        + (SELECT count(*) FROM public.settlement_funding_sources)
                        + (SELECT count(*) FROM public.compensation_ledger)";
    let before = count(pool, funding).await;
    let service = service(database.store().await.expect("运行账号"));
    let version = current_version(pool).await;

    let stranger = service
        .resolve(resolve_input(
            1,
            contract(),
            version,
            OrderResolveAction::CancelUnpaid,
            Uuid::from_u128(0x77),
        ))
        .await
        .expect_err("不是该合同的处置");
    assert_eq!(rejection(&stranger), Some(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!((closures(pool).await, resolutions(pool).await), (0, 0));

    let receipt = service
        .resolve(resolve_input(
            2,
            contract(),
            version,
            OrderResolveAction::CancelUnpaid,
            disposition,
        ))
        .await
        .expect("取消未付");
    let OrderResolveReceipt::ResolutionRecorded { action, .. } = receipt else {
        panic!("应为 resolution_recorded：{receipt:?}");
    };
    assert_eq!(action, OrderResolveAction::CancelUnpaid);
    let (kind, basis): (String, String) = sqlx::query_as(
        "SELECT closure_kind, basis_ref FROM public.consignment_sale_closures WHERE contract_id = $1",
    )
    .bind(contract())
    .fetch_one(pool)
    .await
    .expect("封闭事实");
    assert_eq!(
        (kind.as_str(), basis),
        ("cancelled_unpaid", format!("unpaid-recovery:{DISPOSITION}"))
    );
    assert_eq!(count(pool, funding).await, before, "零资金分录、零补偿");
    assert_eq!((resolutions(pool).await, order_events(pool).await), (1, 1));
    assert_eq!(ledger_of(pool, command(2)).await, Some((200, "ok".to_owned())));

    let replay = service
        .resolve(resolve_input(
            2,
            contract(),
            version,
            OrderResolveAction::CancelUnpaid,
            disposition,
        ))
        .await
        .expect("重放");
    assert_eq!(replay, receipt);
    let twice = service
        .resolve(resolve_input(
            3,
            contract(),
            version,
            OrderResolveAction::CancelUnpaid,
            disposition,
        ))
        .await
        .expect_err("已取消");
    assert_eq!(rejection(&twice), Some(DepositBusinessRejection::DispositionConflict));
    assert_eq!((closures(pool).await, resolutions(pool).await), (1, 1));
}

/// 反例：处置不是“关闭原买方交割”（保留合同）→ 证据不足，不封闭。
#[tokio::test]
async fn postgres_consignment_order_resolve_cancel_unpaid_needs_a_closing_disposition() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let pool = owner.postgres_pool();
    seed_unpaid_sale(pool).await;
    seed_consumption(pool).await;
    succeeds(pool, &disposition_sql(DISPOSITION, ("retain", "unchanged", "forbidden"), true)).await;
    let service = service(database.store().await.expect("运行账号"));

    let error = service
        .resolve(resolve_input(
            1,
            contract(),
            current_version(pool).await,
            OrderResolveAction::CancelUnpaid,
            Uuid::parse_str(DISPOSITION).expect("处置"),
        ))
        .await
        .expect_err("保留合同的处置不得取消");

    assert_eq!(rejection(&error), Some(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!((closures(pool).await, resolutions(pool).await), (0, 0));
}

fn disposition_service(store: &Arc<auction_storage::DatabaseStore>) -> ReturnDispositionService {
    let port: Arc<dyn ReturnDispositionStore> = store.clone();
    ReturnDispositionService::new(port)
}

/// 正例：退款腿被退、精确 Take 已回补原 refund 用途后，`retain_entitlement` 把这笔金额转入同权利的
/// retained 用途——总额守恒、只新增一笔转移；同命令重放原回执；另一条命令再留存同一处置被拒，资金不再变化
/// （零重复资金）。反例：证据不是该合同的现金腿退回处置。
#[tokio::test]
async fn postgres_consignment_order_resolve_retain_entitlement_moves_the_returned_cash_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let pool = owner.postgres_pool();
    let store = Arc::new(database.store().await.expect("运行账号"));
    seed_consumed(pool, &store, Rts { n: 1 }).await;
    let pending = disposition_service(&store).pending(None, 10).await.expect("待处置").items;
    let disposition = disposition_service(&store)
        .resolve(pending[0], AutoRelistSetting { enabled: false, config_digest: [9; 32] })
        .await
        .expect("退回处置")
        .disposition_id;
    let returned = allocation(pool, "refund").await;
    assert!(returned > 0, "精确 Take 已把原支出回补到 refund 用途");
    assert_eq!(allocation(pool, "retained").await, 0);
    let transfers = entries_of(pool, UID, "transfer").await.0;
    // 夹具的 settle_cash 迁移与接管、消费、退回处置都已同事务追加阶段事件；本用例只断言 resolve 恰追加一条。
    let events = order_events(pool).await;
    let service = service(database.store().await.expect("运行账号"));
    let version = current_version(pool).await;

    let stranger = service
        .resolve(resolve_input(
            1,
            contract(),
            version,
            OrderResolveAction::RetainEntitlement,
            Uuid::from_u128(0x77),
        ))
        .await
        .expect_err("不是该合同的处置");
    assert_eq!(rejection(&stranger), Some(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!(allocation(pool, "retained").await, 0);

    let receipt = service
        .resolve(resolve_input(
            2,
            contract(),
            version,
            OrderResolveAction::RetainEntitlement,
            disposition,
        ))
        .await
        .expect("留存");
    let OrderResolveReceipt::ResolutionRecorded { action, .. } = receipt else {
        panic!("应为 resolution_recorded：{receipt:?}");
    };
    assert_eq!(action, OrderResolveAction::RetainEntitlement);
    assert_eq!(
        (allocation(pool, "refund").await, allocation(pool, "retained").await),
        (0, returned),
        "原用途金额整笔转入 retained，总额守恒"
    );
    assert_eq!(entries_of(pool, UID, "transfer").await.0, transfers + 1, "只新增一笔转移");
    assert_eq!((resolutions(pool).await, order_events(pool).await), (1, events + 1));
    assert_eq!(ledger_of(pool, command(2)).await, Some((200, "ok".to_owned())));

    let replay = service
        .resolve(resolve_input(
            2,
            contract(),
            version,
            OrderResolveAction::RetainEntitlement,
            disposition,
        ))
        .await
        .expect("重放");
    assert_eq!(replay, receipt);
    // 留存追加了订单阶段事件，订单版本前进；另一条命令按当前版本提交，只因处置已留存被拒。
    let twice = service
        .resolve(resolve_input(
            3,
            contract(),
            // 首次留存已追加阶段事件、推进了订单版本：以当前版本复见，才能测到处置冲突而非版本过期。
            current_version(pool).await,
            OrderResolveAction::RetainEntitlement,
            disposition,
        ))
        .await
        .expect_err("同一处置已留存");
    assert!(
        matches!(&twice, ConsignmentError::Deposit(DepositBusinessRejection::DispositionConflict)),
        "{twice:?}"
    );
    assert_eq!(
        (allocation(pool, "refund").await, allocation(pool, "retained").await),
        (0, returned)
    );
    assert_eq!(entries_of(pool, UID, "transfer").await.0, transfers + 1);
    assert_eq!(resolutions(pool).await, 1);
}

/// 反例：退款腿被退后想留存，但卖款腿已被真实预留（卖款可能已发）→ `opposite_payment_open`：
/// 不转入 retained，资金、分录、处置与阶段事件零变化。去掉写端锁内的相反付款复验，本用例随之失败。
#[tokio::test]
async fn postgres_consignment_order_resolve_retain_entitlement_refuses_while_the_seller_payment_is_reserved()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let pool = owner.postgres_pool();
    let store = Arc::new(database.store().await.expect("运行账号"));
    seed_consumed(pool, &store, Rts { n: 1 }).await;
    let pending = disposition_service(&store).pending(None, 10).await.expect("待处置").items;
    let disposition = disposition_service(&store)
        .resolve(pending[0], AutoRelistSetting { enabled: false, config_digest: [9; 32] })
        .await
        .expect("退回处置")
        .disposition_id;
    // 卖款腿：分配（S=96）、派发尝试与轮，再经真实的 settle_cash 预留。
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.settlement_funding_allocations
                (id, uid, purpose, obligation_id, consignment_contract_id, amount)
             VALUES ('7e900000-0000-4000-8000-000000000001', '{UID}', 'seller', '{OBLIGATION_2}',
                     '{CONTRACT}', 96);"
        ),
    )
    .await;
    let round =
        round_sql(PAYOUT_ROUND, PAYOUT_DELIVERY, OBLIGATION_2, PAYOUT_ATTEMPT, "cash_payout");
    let payout_round =
        round.replace("'2026-01-01T04:00:00Z', 0, 101,", "'2026-01-01T04:00:00Z', 96, 0,");
    assert_ne!(payout_round, round, "夹具应把轮的 Credits 改为卖款 96");
    succeeds(pool, &(send_sql(PAYOUT_DELIVERY, OBLIGATION_2, PAYOUT_MID, 0) + &payout_round)).await;
    store
        .test_settle_cash(SettleCash {
            mutation: mutation(32),
            contract_id: id(CONTRACT),
            obligation_id: id(OBLIGATION_2),
            delivery_id: id(PAYOUT_DELIVERY),
            leg: CashLeg::Seller,
            phase: CashPhase::Reserve,
            amount: 96,
        })
        .await
        .expect("预留卖款");
    let returned = allocation(pool, "refund").await;
    let transfers = entries_of(pool, UID, "transfer").await;
    // 夹具的 settle_cash 迁移与接管、消费、退回处置都已同事务追加阶段事件；本用例只断言 resolve 不再追加。
    let events = order_events(pool).await;
    let service = service(database.store().await.expect("运行账号"));

    let error = service
        .resolve(resolve_input(
            1,
            contract(),
            current_version(pool).await,
            OrderResolveAction::RetainEntitlement,
            disposition,
        ))
        .await
        .expect_err("卖款已预留");

    assert_eq!(rejection(&error), Some(DepositBusinessRejection::OppositePaymentOpen));
    assert_eq!(
        (allocation(pool, "refund").await, allocation(pool, "retained").await),
        (returned, 0),
        "没有转入 retained"
    );
    assert_eq!(entries_of(pool, UID, "transfer").await, transfers);
    assert_eq!((resolutions(pool).await, order_events(pool).await), (0, events));
    assert_eq!(ledger_of(pool, command(1)).await, Some((409, "opposite_payment_open".to_owned())));
}
