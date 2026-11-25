//! 现金腿状态迁移：金额与资金、命令幂等、外发未知不推导返还。

use auction_application::settlement::{CashPhase, FundingError, FundingMutation};
use sqlx::PgPool;

use super::super::ledger136::fixture::PRICE;
use super::fixture::{UID, cash, entries_of, mutation, seed_refund_ready, seed_refund_spent};
use crate::support::TestDatabase;

async fn refund_allocation(pool: &PgPool) -> (String, i64) {
    sqlx::query_as(
        "SELECT state, amount FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = 'refund'",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("读取退款分配")
}

#[tokio::test]
async fn postgres_consignment_funding_cash_leg_moves_only_forward_and_replays_without_new_entries()
{
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_refund_ready(pool).await;

    // 反例：金额与冻结的派发轮不符，不预留、不写分录。
    let wrong = store.test_settle_cash(cash(1, CashPhase::Reserve, PRICE - 1)).await;
    assert!(matches!(wrong, Err(FundingError::Conflict)));
    assert_eq!(refund_allocation(pool).await, ("available".to_owned(), PRICE));

    // 正例：reserve 一次；同命令重放不新增分录；同 ID 异请求冲突。
    let reserved = store.test_settle_cash(cash(1, CashPhase::Reserve, PRICE)).await.expect("预留");
    assert!(!reserved.replayed);
    assert!(
        store.test_settle_cash(cash(1, CashPhase::Reserve, PRICE)).await.expect("重放").replayed
    );
    let mut forged = cash(1, CashPhase::Reserve, PRICE);
    forged.mutation = FundingMutation { request_digest: [0xee; 32], ..mutation(1) };
    assert!(matches!(store.test_settle_cash(forged).await, Err(FundingError::Conflict)));
    assert_eq!(entries_of(pool, UID, "reserve").await, (1, PRICE));
    // 反例：同一分配已被预留，另一条命令不能再预留（防双派）。
    let twice = store.test_settle_cash(cash(9, CashPhase::Reserve, PRICE)).await;
    assert!(matches!(twice, Err(FundingError::Unavailable)));
    assert_eq!(entries_of(pool, UID, "reserve").await, (1, PRICE));

    // 反例：未起写（reserved）不能直接算已外达；in_flight 之后才能 spent。
    let early = store.test_settle_cash(cash(2, CashPhase::Spent, PRICE)).await;
    assert!(matches!(early, Err(FundingError::Conflict)));
    store.test_settle_cash(cash(3, CashPhase::InFlight, PRICE)).await.expect("起写");
    store.test_settle_cash(cash(4, CashPhase::Spent, PRICE)).await.expect("已证外达");
    assert_eq!(refund_allocation(pool).await, ("spent".to_owned(), PRICE));
    assert_eq!(entries_of(pool, UID, "spent").await, (1, PRICE));
    // 已支出的钱不会因任何迁移回到 available：只有精确 Take 的回补可以（见 blockers）。
    let back = store.test_settle_cash(cash(5, CashPhase::Reserve, PRICE)).await;
    assert!(matches!(back, Err(FundingError::Unavailable)));
}

#[tokio::test]
async fn postgres_consignment_funding_no_op_cash_command_still_pins_its_request() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_refund_spent(pool, &store).await;
    let version = account_version(pool).await;

    // 已在 spent：新命令是无变化的迁移，仍登记账本且不新增分录、不推进账户版本。
    let noop = store.test_settle_cash(cash(7, CashPhase::Spent, PRICE)).await.expect("无变化");
    assert!(noop.replayed);
    assert_eq!(entries_of(pool, UID, "spent").await, (1, PRICE));
    assert_eq!(account_version(pool).await, version);
    let ledgered: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_funding_commands WHERE command_id = $1",
    )
    .bind(mutation(7).command_id)
    .fetch_one(pool)
    .await
    .expect("读取命令账本");
    assert_eq!(ledgered, 1);

    // 同 ID 同请求回原结果；同 ID 异请求冲突，不能被当成成功重放。
    assert!(store.test_settle_cash(cash(7, CashPhase::Spent, PRICE)).await.expect("重放").replayed);
    let mut forged = cash(7, CashPhase::Spent, PRICE);
    forged.mutation = FundingMutation { request_digest: [0xee; 32], ..mutation(7) };
    assert!(matches!(store.test_settle_cash(forged).await, Err(FundingError::Conflict)));
}

async fn account_version(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT state_version FROM public.settlement_funding_accounts WHERE uid = $1",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("读取账户版本")
}

#[tokio::test]
async fn postgres_consignment_funding_unknown_send_keeps_the_hold() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_refund_ready(pool).await;
    store.test_settle_cash(cash(1, CashPhase::Reserve, PRICE)).await.expect("预留");
    store.test_settle_cash(cash(2, CashPhase::InFlight, PRICE)).await.expect("起写");
    store.test_settle_cash(cash(3, CashPhase::Unknown, PRICE)).await.expect("结果未知");
    // 未知保留全部占用：不能回到 available，也不能留存为 retained。
    assert_eq!(refund_allocation(pool).await, ("unknown".to_owned(), PRICE));
    let retained = store.test_settle_cash(cash(4, CashPhase::Retained, PRICE)).await;
    assert!(matches!(retained, Err(FundingError::Conflict)));
    assert_eq!(entries_of(pool, UID, "return").await, (0, 0));
}
