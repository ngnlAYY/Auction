//! 现金退件的 UID 级 opening 阻断：Take 起写同事务建立、只随原资金消费解除、两种锁序下 opening 都看得见。

use auction_application::settlement::{FundingCommitOutcome, FundingError};
use auction_domain::consignment::funding::BlockedLeg;
use auction_domain::consignment::funding::{FundingMode, OpeningBlock};
use sqlx::PgPool;

use super::super::contracts::fixture::CONTRACT;
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{Intake, PRICE, REFUND_DELIVERY};
use super::fixture::{
    CONSUMPTION, UID, cash_refund_intake, consumption_sql, entries_of, id, ledger_counts, mutation,
    refund_block, seed_refund_spent, seed_unconsumed_cash_return_block, try_reconcile,
};
use crate::support::TestDatabase;

fn rejected_as_unproven(outcome: &Result<FundingCommitOutcome, FundingError>) -> bool {
    matches!(outcome, Ok(FundingCommitOutcome::RolledBack(FundingError::Unproven)))
}

async fn open_blocks(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_cash_return_blocks WHERE state = 'open'",
    )
    .fetch_one(pool)
    .await
    .expect("读取阻断数")
}

#[tokio::test]
async fn postgres_consignment_funding_cash_take_start_blocks_opening_until_one_full_return() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let (intake, _) = seed_unconsumed_cash_return_block(pool, &store).await;
    assert_eq!(open_blocks(pool).await, 1, "起写同事务建立阻断");

    // 反例：opening 最终写被拒；零 source、零 issue，账本边界不动。
    let before = ledger_counts(pool, UID).await;
    assert!(rejected_as_unproven(&try_reconcile(&store, pool, UID).await));
    assert_eq!(ledger_counts(pool, UID).await, before, "被拒的 opening 不写任何来源或分录");
    assert_eq!(entries_of(pool, UID, "issue").await, (0, 0));

    // 正例：补齐精确 Take 与消费后，恰一笔全额 return，原用途恢复，阻断同事务解除。
    let ids = [id(CONTRACT), id(&intake.intake()), id(CONSUMPTION), id(REFUND_DELIVERY)];
    store
        .test_return_cash(
            &consumption_sql(&intake),
            mutation(10),
            ids,
            BlockedLeg::Refund,
            PRICE,
            [7; 32],
        )
        .await
        .expect("消费同事务回补");
    assert_eq!(entries_of(pool, UID, "return").await, (1, PRICE), "恰一笔全额 return");
    let (state, delivery, amount): (String, Option<uuid::Uuid>, i64) = sqlx::query_as(
        "SELECT state, delivery_id, amount FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = 'refund'",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("读取退款分配");
    assert_eq!((state.as_str(), delivery, amount), ("available", None, PRICE));
    assert_eq!(open_blocks(pool).await, 0);

    // 重放：同命令不产生第二次恢复；换命令复见同一消费为冲突。
    store
        .test_return_cash("", mutation(10), ids, BlockedLeg::Refund, PRICE, [7; 32])
        .await
        .expect("同命令重放");
    let other =
        store.test_return_cash("", mutation(11), ids, BlockedLeg::Refund, PRICE, [7; 32]).await;
    assert!(matches!(other, Err(FundingError::Conflict)));
    assert_eq!(entries_of(pool, UID, "return").await, (1, PRICE));

    // 核清后门禁放开：opening 不再因阻断被拒（之后在预览摘要比对处失败，不是 Unproven）。
    let released = try_reconcile(&store, pool, UID).await;
    assert!(
        matches!(&released, Ok(FundingCommitOutcome::RolledBack(FundingError::Conflict))),
        "{released:?}"
    );
}

#[tokio::test]
async fn postgres_consignment_funding_failed_return_keeps_block_and_unknown_return_is_not_released()
{
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let (intake, _) = seed_unconsumed_cash_return_block(pool, &store).await;
    let ids = [id(CONTRACT), id(&intake.intake()), id(CONSUMPTION), id(REFUND_DELIVERY)];

    // Take 成功（起写已提交）但资金回补事务失败回滚：阻断仍在，opening 仍被拒。
    let wrong_amount = store
        .test_return_cash(
            &consumption_sql(&intake),
            mutation(10),
            ids,
            BlockedLeg::Refund,
            PRICE - 1,
            [7; 32],
        )
        .await;
    assert!(matches!(wrong_amount, Err(FundingError::Conflict)));
    assert_eq!(open_blocks(pool).await, 1);
    assert_eq!(entries_of(pool, UID, "return").await, (0, 0));
    assert!(rejected_as_unproven(&try_reconcile(&store, pool, UID).await));

    // 未知的旧退件：没有阻断行，不因“无消费行”放行，也不写任何资金。
    let legacy = Intake { n: 2, leg: "cash_payout" };
    succeeds(pool, &legacy.sql()).await;
    let legacy_ids = [id(CONTRACT), id(&legacy.intake()), id(CONSUMPTION), id(REFUND_DELIVERY)];
    let unknown = store
        .test_return_cash("", mutation(12), legacy_ids, BlockedLeg::Payout, PRICE, [7; 32])
        .await;
    assert!(matches!(unknown, Err(FundingError::Unproven)));
    assert_eq!(entries_of(pool, UID, "return").await, (0, 0));
}

#[tokio::test]
async fn postgres_consignment_funding_opening_committed_first_still_sees_later_take_block() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_refund_spent(pool, &store).await;
    // opening 先提交：边界已建立，之后的现金 Take 只能发生在该边界之后。
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.settlement_funding_sources (id, uid, kind, source_key, amount, proof_digest)
             VALUES ('c6c30000-0000-4000-8000-000000000001', '{UID}', 'reconciliation',
                     'c6c30000-0000-4000-8000-000000000001', 0, decode(repeat('05', 32), 'hex'));"
        ),
    )
    .await;
    let intake = cash_refund_intake();
    succeeds(pool, &intake.sql()).await;
    store
        .test_open_cash_return_block(&intake.start_sql(), &refund_block(&intake))
        .await
        .expect("起写在已建边界之后建立阻断");
    let before = ledger_counts(pool, UID).await;
    assert!(rejected_as_unproven(&try_reconcile(&store, pool, UID).await));
    assert_eq!(ledger_counts(pool, UID).await, before);
}

#[tokio::test]
async fn postgres_consignment_funding_block_requires_same_transaction_write_start() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_refund_spent(pool, &store).await;
    let intake = cash_refund_intake();
    succeeds(pool, &intake.sql()).await;
    // 起写事实已在更早的事务提交，阻断没有随它同事务建立：失败关闭，不留阻断。
    succeeds(pool, &intake.start_sql()).await;
    let late = store.test_open_cash_return_block("", &refund_block(&intake)).await;
    assert!(matches!(late, Err(FundingError::Conflict)));
    assert_eq!(open_blocks(pool).await, 0);
    // 阻断记录的原模式与账户模式不符同样失败关闭。
    let wrong_mode = OpeningBlock::new(
        UID.to_owned(),
        id(&intake.intake()),
        BlockedLeg::Refund,
        id(REFUND_DELIVERY),
        PRICE,
        FundingMode::RecordOnly,
    )
    .expect("阻断记录");
    let mismatch = store.test_open_cash_return_block("", &wrong_mode).await;
    assert!(matches!(mismatch, Err(FundingError::Conflict)));
    // 起写工作取自 intake：不存在的 intake 没有可绑定的起写，同样失败关闭。
    let unknown = refund_block(&Intake { n: 2, leg: "cash_refund" });
    let missing = store.test_open_cash_return_block("", &unknown).await;
    assert!(matches!(missing, Err(FundingError::Conflict)));
    assert_eq!(open_blocks(pool).await, 0);
}
