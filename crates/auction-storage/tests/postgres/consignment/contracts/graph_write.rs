//! 合同义务图写入原语：单事务原子、同合同重放与异条款冲突（不变量 1、2、3、6）。
//!
//! 被测端口经运行账号执行，证明运行账号的 SELECT / INSERT 授权足够；夹具与计数用维护账号。

use auction_application::consignment::{ConsignmentError, ConsignmentGraphStore};
use auction_domain::consignment::{
    ConsignmentContract, ConsignmentContractCore, ConsignmentObligationGraph,
    ConsignmentSalePolicy, FeeBps, FundingAccountingMode, ListingPrice, ListingVersion,
    SaleFundingFreeze, SaleTerms, SaleTermsInput, WithdrawalTerms, build_consignment_obligations,
};
use auction_domain::{CommandId, ConsignmentContractId, SettlementWorld, UserId};
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::{ASSET, DECIDE, LISTING, POSITION, USER_1, USER_2, succeeds};
use super::fixture::{CONTRACT, WINDOW, seed_isekai_custody, seed_main_custody};
use crate::support::TestDatabase;

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("夹具 UUID")
}

fn at(text: &str) -> Timestamp {
    Timestamp::parse_from_rfc3339(text).expect("夹具时间")
}

/// 与 5M 夹具同一份代管 position 上的合同核心；创建时刻带纳秒，用来证明重放按数据库微秒精度比较。
pub(super) fn core(world: SettlementWorld, origin_command: &str) -> ConsignmentContractCore {
    ConsignmentContractCore {
        contract_id: ConsignmentContractId::from_uuid(uuid(CONTRACT)),
        origin_command_id: CommandId::from_uuid(uuid(origin_command)),
        listing_id: uuid(LISTING),
        position_id: uuid(POSITION),
        generation: 1,
        custody_equipment_id: uuid(ASSET),
        world,
        canonical_eid: "42".to_owned(),
        season_window_id: (world == SettlementWorld::Isekai).then(|| uuid(WINDOW)),
        seller_user_id: UserId::from_uuid(uuid(USER_1)),
        frozen_seller_identity: "FakeSellerOne".to_owned(),
        created_at: at("2026-01-02T00:00:00.123456789Z"),
    }
}

pub(super) fn sale_terms(price: i64) -> SaleTerms {
    SaleTerms::new(SaleTermsInput {
        buyer_user_id: UserId::from_uuid(uuid(USER_2)),
        frozen_buyer_identity: "FakeSellerTwo".to_owned(),
        listing_version: ListingVersion::new(4).expect("版本"),
        price: ListingPrice::new(price).expect("标价"),
        fee_bps: FeeBps::new(500).expect("费率"),
        payment_deadline_at: at("2026-01-09T00:00:00Z"),
        policy: ConsignmentSalePolicy::new(7, 3, 5).expect("策略"),
        funding: SaleFundingFreeze {
            uid: "7001".to_owned(),
            mode: FundingAccountingMode::StrictCash,
            boundary_at: at("2026-01-02T00:00:00Z"),
            boundary_source_id: None,
            // 非零版本：重放逐项比较时证明 000136 的冻结列被写入并读回。
            account_version: 3,
        },
    })
    .expect("成交条款")
}

pub(super) fn sale_graph(world: SettlementWorld, price: i64) -> ConsignmentObligationGraph {
    let contract =
        ConsignmentContract::Sale { core: core(world, DECIDE), terms: sale_terms(price) };
    build_consignment_obligations(&contract).expect("义务图")
}

pub(super) fn withdrawal_graph() -> ConsignmentObligationGraph {
    let contract = ConsignmentContract::Withdrawal {
        core: core(SettlementWorld::Main, DECIDE),
        terms: WithdrawalTerms { intent_id: None, predecessor_contract_id: None },
    };
    build_consignment_obligations(&contract).expect("义务图")
}

/// Sale 的资金账户外键需要的账户行。
pub(super) async fn seed_funding(pool: &PgPool) {
    succeeds(
        pool,
        "INSERT INTO public.settlement_funding_accounts (uid, status)
         VALUES ('7001', 'ready') ON CONFLICT DO NOTHING;",
    )
    .await;
}

/// 合同、子项、义务与履约行的行数。
pub(super) async fn graph_rows(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.consignment_contracts),
                (SELECT count(*) FROM public.consignment_sales)
                    + (SELECT count(*) FROM public.consignment_withdrawals),
                (SELECT count(*) FROM public.settlement_obligations WHERE contract_id IS NOT NULL),
                (SELECT count(*) FROM public.settlement_obligation_fulfillments AS fulfillment
                 JOIN public.settlement_obligations AS obligation
                   ON obligation.id = fulfillment.obligation_id
                 WHERE obligation.contract_id IS NOT NULL)",
    )
    .fetch_one(pool)
    .await
    .expect("读取合同图行数")
}

#[tokio::test]
async fn postgres_consignment_graph_write_commits_whole_graph_then_replays() {
    // 正例：三种固定形状都由运行账号一次写入，重放返回同一图且不增行。
    for (graph, isekai, expected) in [
        (sale_graph(SettlementWorld::Main, 1000), false, (1, 1, 3, 3)),
        (sale_graph(SettlementWorld::Isekai, 1000), true, (1, 1, 4, 4)),
        (withdrawal_graph(), false, (1, 1, 1, 1)),
    ] {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let runtime = database.store().await.expect("以运行账号连接");
        let pool = owner.postgres_pool();
        if isekai {
            seed_isekai_custody(pool, "transaction_timestamp() + interval '10 days'").await;
        } else {
            seed_main_custody(pool).await;
        }
        seed_funding(pool).await;

        let first = runtime.write_graph(graph.clone()).await.expect("首次写入");
        assert!(!first.replayed);
        assert_eq!(
            first.obligation_ids,
            graph.obligations.iter().map(|obligation| obligation.id).collect::<Vec<_>>()
        );
        assert_eq!(graph_rows(pool).await, expected);

        let again = runtime.write_graph(graph.clone()).await.expect("重放");
        assert!(again.replayed);
        assert_eq!(again.contract_id, first.contract_id);
        assert_eq!(again.obligation_ids, first.obligation_ids);
        assert_eq!(graph_rows(pool).await, expected, "重放不增行");
    }
}

#[tokio::test]
async fn postgres_consignment_graph_write_rejects_same_contract_with_different_terms() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    seed_funding(pool).await;
    runtime.write_graph(sale_graph(SettlementWorld::Main, 1000)).await.expect("首次写入");
    let before = graph_rows(pool).await;

    // 反例：同合同 ID、同来源命令、不同价格（图本身合法）返回冲突且不改任何行。
    let error = runtime
        .write_graph(sale_graph(SettlementWorld::Main, 2000))
        .await
        .expect_err("异条款必须冲突");
    assert_eq!(error, ConsignmentError::CommandIdConflict);

    // 反例：另一个合同 ID 复用同一来源命令，同样冲突。
    let mut other = sale_graph(SettlementWorld::Main, 1000);
    let other_contract = ConsignmentContract::Sale {
        core: ConsignmentContractCore {
            contract_id: ConsignmentContractId::from_uuid(Uuid::new_v4()),
            ..other.contract.core().clone()
        },
        terms: sale_terms(1000),
    };
    other = build_consignment_obligations(&other_contract).expect("另一份图");
    let error = runtime.write_graph(other).await.expect_err("同来源命令不同合同必须冲突");
    assert_eq!(error, ConsignmentError::CommandIdConflict);
    assert_eq!(graph_rows(pool).await, before);
    let price: i64 = sqlx::query_scalar("SELECT price FROM public.consignment_sales")
        .fetch_one(pool)
        .await
        .expect("读取已存价格");
    assert_eq!(price, 1000, "冲突不得改写既有合同");
}

#[tokio::test]
async fn postgres_consignment_graph_write_rolls_back_every_row_on_failure() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;

    // 反例一：资金账户不存在，合同头已写入后子项失败，合同头必须随事务回滚。
    runtime
        .write_graph(sale_graph(SettlementWorld::Main, 1000))
        .await
        .expect_err("缺资金账户必须失败");
    assert_eq!(graph_rows(pool).await, (0, 0, 0, 0));

    // 反例二：来源命令不在账本，所有行都已写入后由延迟外键在 COMMIT 失败，整图回滚。
    seed_funding(pool).await;
    let contract = ConsignmentContract::Sale {
        core: core(SettlementWorld::Main, "41000000-0000-5000-8000-0000000000ee"),
        terms: sale_terms(1000),
    };
    let dangling = build_consignment_obligations(&contract).expect("来源命令缺失的图");
    runtime.write_graph(dangling).await.expect_err("来源命令缺失必须在提交时失败");
    assert_eq!(graph_rows(pool).await, (0, 0, 0, 0));
}

#[tokio::test]
async fn postgres_consignment_graph_write_rejects_tampered_graph_before_any_row() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    seed_funding(pool).await;

    // 反例：卖款金额被篡改为 S + 1，写前重算即拒绝，不进入事务。
    let mut tampered = sale_graph(SettlementWorld::Main, 1000);
    let payout = tampered
        .obligations
        .iter_mut()
        .find(|obligation| {
            obligation.kind == auction_domain::SettlementObligationKind::SellerPayout
        })
        .expect("卖款义务");
    payout.amount = auction_domain::SettlementAmount::new(951).expect("金额");
    let error = runtime.write_graph(tampered).await.expect_err("篡改图必须被拒绝");
    assert_eq!(error, ConsignmentError::InvalidDepositRequest);
    assert_eq!(graph_rows(pool).await, (0, 0, 0, 0));
}
