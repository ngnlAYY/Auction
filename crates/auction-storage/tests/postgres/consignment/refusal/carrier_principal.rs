//! isekai 处罚与本金（D9 §11.1“处罚及季界”）：同买家三张不同合同的季前未付载体退回，第三封季后才补证、Take
//! 一直待核。补证提交即恰一项处罚、三个 `return_fact_id` 构成项、一项账号工作，新购被拒，第三封本金回补为 0；
//! 迟到的精确 Take 只结清本金 1，不增拒收或处罚。反例：第三封恰在关季等号退回，即使会凑足阈值也不处罚，
//! Take 后同样只结清 1。
//!
//! 保护不变量 2（买家锁内判定）、4（外部结果未知时资金不动）、6（不补写矛盾事实）、8（裁决与拒收只追加）。
//! 严格现金下本金结清即回原自有；record_only 的核销子账分支见 `dispatch/cash_rounds.rs`。

use sqlx::PgPool;

use super::fixture::{buyer, command, count, seller};
use super::stock::{IsekaiStock, OWN, ThirdReturn, own, principal_effects, seed_isekai_sanction};
use crate::support::TestDatabase;

/// 拒收侧的不可变事实快照：(返回事实, 裁决, 拒收, 处罚, 构成项, 账号工作)。
async fn refusal_facts(owner: &PgPool) -> [i64; 6] {
    let mut counts = [0; 6];
    for (slot, table) in counts.iter_mut().zip([
        "consignment_return_facts",
        "consignment_return_fact_verdicts",
        "consignment_refusals",
        "consignment_sanctions",
        "consignment_sanction_members",
        "consignment_account_work",
    ]) {
        *slot = count(owner, table).await;
    }
    counts
}

async fn purchase_blocked(store: &auction_storage::DatabaseStore, lock_key: u128) -> bool {
    store.test_purchase_gate(command(lock_key), buyer(), seller(), None).await.expect("阻断读")
}

/// 迟到的精确 Take 只结清本金 1：回补恰一次、阻断解除、自有恢复，拒收侧事实逐项不变；同命令重放无新增。
async fn assert_late_take_settles_only_the_principal(
    owner: &PgPool,
    store: &auction_storage::DatabaseStore,
    stock: &IsekaiStock,
) {
    let before = refusal_facts(owner).await;
    stock.consume_third_take(store, 0x51).await.expect("迟到的精确 Take 与资金消费");
    assert_eq!(principal_effects(owner).await, (1, 1, 1, 0));
    assert_eq!(own(owner).await, OWN, "严格现金：本金 1 回原自有");
    assert_eq!(refusal_facts(owner).await, before, "Take 不增拒收或处罚");
    stock.replay_third_take(store, 0x51).await.expect("同命令重放");
    assert_eq!(principal_effects(owner).await, (1, 1, 1, 0));
    assert_eq!(own(owner).await, OWN);
}

#[tokio::test]
async fn postgres_consignment_refusal_isekai_late_proof_sanctions_once_while_principal_waits_for_take()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let owner = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let stock = seed_isekai_sanction(owner, &store, ThirdReturn::BeforeClose).await;
    let sanction = stock.sanction.as_ref().expect("第三份补证应触发处罚");

    // 补证提交即恰一项处罚、三个不同合同的构成项、一项账号工作；判定时刻在季界之后（季后补证），
    // 最早一份在 t0 时为 89 天（仍在左开右闭的 90 天窗内）。
    assert_eq!(count(owner, "consignment_sanctions").await, 1);
    assert_eq!(count(owner, "consignment_account_work").await, 1);
    let (members, contracts, after_close, earliest_age): (i64, i64, bool, bool) = sqlx::query_as(
        "SELECT count(*), count(DISTINCT fact.contract_id), bool_and(sanction.t0 > $2),
                bool_and(sanction.t0 - $3 BETWEEN interval '89 days' AND interval '89 days 1 hour')
         FROM public.consignment_sanction_members AS member
         JOIN public.consignment_return_facts AS fact ON fact.return_fact_id = member.return_fact_id
         JOIN public.consignment_sanctions AS sanction ON sanction.id = member.sanction_id
         WHERE member.sanction_id = $1",
    )
    .bind(sanction.sanction_id)
    .bind(stock.close)
    .bind(stock.earliest_returned_at)
    .fetch_one(owner)
    .await
    .expect("构成项");
    assert_eq!((members, contracts, after_close, earliest_age), (3, 3, true, true));

    // 新购被拒；Take 待核期间第三封本金回补为 0：无回补、阻断仍在、自有仍是借出后的余额。
    assert!(purchase_blocked(&store, 900).await, "达阈值同事务阻断新购");
    assert_eq!(principal_effects(owner).await, (0, 0, 0, 1));
    assert_eq!(own(owner).await, OWN - 1);

    assert_late_take_settles_only_the_principal(owner, &store, &stock).await;
    assert!(purchase_blocked(&store, 901).await, "Take 完成不解除处罚阻断");
}

/// 反例：第三封恰在关季等号退回。可交付证据与时刻矛盾，只待判：不写第三份拒收、不处罚、不阻断新购；
/// Take 后只结清本金 1。
#[tokio::test]
async fn postgres_consignment_refusal_isekai_return_at_close_never_sanctions_and_only_settles_principal()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let owner = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let stock = seed_isekai_sanction(owner, &store, ThirdReturn::AtClose).await;

    assert!(stock.sanction.is_none(), "关季等号起退回不凑数");
    let third: String = sqlx::query_scalar(
        "SELECT string_agg(verdict.verdict || ':' || verdict.reason, ',' ORDER BY verdict.evaluated_at, verdict.id)
         FROM public.consignment_return_fact_verdicts AS verdict
         JOIN public.consignment_return_facts AS fact ON fact.return_fact_id = verdict.return_fact_id
         WHERE fact.intake_id = $1",
    )
    .bind(stock.third_intake_id)
    .fetch_one(owner)
    .await
    .expect("第三份裁决");
    assert_eq!(third, "pending:deliverability_disputed", "补证同结论不追加裁决");
    assert_eq!(count(owner, "consignment_refusals").await, 2);
    assert_eq!(count(owner, "consignment_sanctions").await, 0);
    assert_eq!(count(owner, "consignment_account_work").await, 0);
    assert!(!purchase_blocked(&store, 910).await);
    assert_eq!(principal_effects(owner).await, (0, 0, 0, 1));

    assert_late_take_settles_only_the_principal(owner, &store, &stock).await;
    assert_eq!(count(owner, "consignment_sanctions").await, 0);
}
