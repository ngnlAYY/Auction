//! COD 实收认证：同一经济实收的单一结果（G = A + U）、整笔回滚、世界与收款腿互校、重放与并发。

use auction_application::settlement::{CertifyCod, CodLeg, FundingError};
use auction_domain::SettlementWorld;
use auction_storage::DatabaseStore;
use uuid::Uuid;

use super::cod_fixture::{
    COMPENSATION_ENTRY, GROSS, Ledger, OPENING_ENTRY, QUARANTINE, Shape, allocation_amount,
    certify, ledger, quarantine_amount, register, seed,
};
use crate::support::TestDatabase;

/// 一个独立数据库：维护账号池做夹具与断言，运行账号 store 执行被测入口。
pub(super) struct Env {
    _database: TestDatabase,
    pub(super) owner: DatabaseStore,
    pub(super) store: DatabaseStore,
}

impl Env {
    pub(super) async fn new(shape: Shape) -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let store = database.store().await.expect("以运行账号连接");
        seed(owner.postgres_pool(), shape).await;
        Self { _database: database, owner, store }
    }

    pub(super) fn pool(&self) -> &sqlx::PgPool {
        self.owner.postgres_pool()
    }

    /// 登记 opening 份额并返回实收业务键的归属 ID。
    pub(super) async fn seal(&self, entry: &str, end: i64) -> Uuid {
        self.store
            .test_register_opening_attribution(register(1, &[(entry, 0, end)]))
            .await
            .expect("登记份额")
            .attribution_id
    }

    /// 认证必须被拒绝，且资金账本、收据、funded 与履约 / 下游状态与失败前逐字相同。
    pub(super) async fn assert_rejected(
        &self,
        command: CertifyCod,
        expected: fn(&FundingError) -> bool,
    ) {
        let before = ledger(self.pool()).await;
        let error = self.store.test_certify_cod(command).await.expect_err("缺陷必须被拒绝");
        assert!(expected(&error), "拒绝分类不符：{error:?}");
        assert_eq!(
            ledger(self.pool()).await,
            before,
            "整笔回滚：无孤立 satisfied、下游 ready 或半笔收入"
        );
    }
}

/// 全含 / 部分含 / 不含三例：新增发行恰为 U = G - A，原 opening 借方恰为 A，最终 S/F/1 = 96/5/1。
async fn certify_case(included: i64) {
    let env = Env::new(Shape::STRICT).await;
    let pool = env.pool();
    let attribution =
        if included > 0 { Some(env.seal(OPENING_ENTRY, included).await) } else { None };
    let before = ledger(pool).await;
    let opening_before = quarantine_amount(pool).await;
    let compensation_before = allocation_amount(pool, "compensation", false).await;

    let receipt = env.store.test_certify_cod(certify(2, attribution)).await.expect("认证 COD");
    assert!(!receipt.replayed);
    assert_eq!(
        (receipt.split.gross(), receipt.split.opening_included(), receipt.split.new_issue()),
        (GROSS, included, GROSS - included)
    );

    let after = ledger(pool).await;
    assert_eq!(after.issued - before.issued, GROSS - included, "新增发行 = U");
    assert_eq!(opening_before - quarantine_amount(pool).await, included, "原 opening 借方 = A");
    assert_eq!(allocation_amount(pool, "seller", true).await, 96);
    assert_eq!(allocation_amount(pool, "fee", true).await, 5);
    assert_eq!(allocation_amount(pool, "proprietary", false).await, 1, "载体本金归原 UID 自有");
    assert_eq!(allocation_amount(pool, "consignment_transit", true).await, 0, "过渡用途回零");
    assert_eq!(allocation_amount(pool, "compensation", false).await, compensation_before);
    assert_eq!((after.compensation_pool, after.compensation_rows), (0, 0), "补偿池零增量");
    assert_eq!(
        (after.receipts, after.funded, after.fee_facts, after.slice_uses),
        (1, 1, 1, i64::from(included > 0))
    );
    assert_eq!(
        (after.payment_state.as_str(), after.handoff_state.as_str()),
        ("satisfied", "ready")
    );
    // 回执与审计同事务写入：认证（及可选的份额登记）各一行回执与一行审计，回执形状是 payment_funded。
    let commands = 1 + i64::from(included > 0);
    assert_eq!((after.receipt_commands, after.audits), (commands, commands));
    let (status, disposition, target_kind, new_issue): (i16, String, String, i64) = sqlx::query_as(
        "SELECT result_status, result_body->>'disposition', target_kind,
                (result_body->>'new_issue')::bigint
         FROM public.consignment_commands WHERE operation = 'certify_cod'",
    )
    .fetch_one(pool)
    .await
    .expect("读取认证回执");
    assert_eq!(
        (status, disposition.as_str(), target_kind.as_str(), new_issue),
        (200, "payment_funded", "obligation", GROSS - included)
    );

    // 重放：同命令、换命令 ID 复见同一实收，都读原结果，资金与履约零变化。
    let again = env.store.test_certify_cod(certify(2, attribution)).await.expect("同命令重放");
    assert!(again.replayed);
    assert_eq!(again.receipt_source_id, receipt.receipt_source_id);
    let other = env.store.test_certify_cod(certify(3, attribution)).await.expect("换命令重放");
    assert!(other.replayed);
    assert_eq!(other.receipt_source_id, receipt.receipt_source_id);
    // 换命令 ID 是另一条被接受的命令：只多一行回执与一行审计，资金与履约零变化。
    assert_eq!(
        ledger(pool).await,
        Ledger { receipt_commands: commands + 1, audits: commands + 1, ..after.clone() },
        "重放零新增"
    );
    let replayed = ledger(pool).await;

    // 异拆分 / 异额复见同一实收，以及同命令 ID 异请求，都是冲突，不读原结果也不写新结果。
    let mut other_gross = certify(4, attribution);
    other_gross.gross = GROSS + 1;
    assert!(matches!(env.store.test_certify_cod(other_gross).await, Err(FundingError::Conflict)));
    assert_eq!(ledger(pool).await, replayed, "冲突零新增");
    let mut changed = certify(2, attribution);
    changed.mutation.request_digest = [9; 32];
    assert!(matches!(env.store.test_certify_cod(changed).await, Err(FundingError::Conflict)));
    assert_eq!(ledger(pool).await, replayed, "同 ID 异请求不入账：无回执、无审计");
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_splits_gross_between_opening_and_new_issue() {
    for included in [GROSS, 60, 0] {
        certify_case(included).await;
    }
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_rolls_back_whole_receipt_on_any_defect() {
    // 错 allocation：份额落在补偿用途上，拍卖补偿余额不能借用。
    let env = Env::new(Shape::STRICT).await;
    let sealed = env.seal(COMPENSATION_ENTRY, 60).await;
    env.assert_rejected(certify(2, Some(sealed)), |e| matches!(e, FundingError::Unproven)).await;
    assert_eq!(allocation_amount(env.pool(), "compensation", false).await, 100);

    // 已花余额：登记后原隔离余额被花掉一部分，不足以借出 A。
    let env = Env::new(Shape::STRICT).await;
    let sealed = env.seal(OPENING_ENTRY, 60).await;
    sqlx::query("UPDATE public.settlement_funding_allocations SET amount = 10 WHERE id = $1::uuid")
        .bind(QUARANTINE)
        .execute(env.pool())
        .await
        .expect("花掉隔离余额");
    env.assert_rejected(certify(2, Some(sealed)), |e| matches!(e, FundingError::Unavailable)).await;

    // 无法逐笔解释：声明了归属却没有封存份额；封存了份额却声称全部未含。
    let env = Env::new(Shape::STRICT).await;
    env.assert_rejected(certify(2, Some(Uuid::from_bytes([7; 16]))), |e| {
        matches!(e, FundingError::Unproven)
    })
    .await;
    env.seal(OPENING_ENTRY, 60).await;
    env.assert_rejected(certify(2, None), |e| matches!(e, FundingError::Conflict)).await;

    // 无远端付款明细：没有付款证据不能转 satisfied。
    let env = Env::new(Shape { remote_payment: false, ..Shape::STRICT }).await;
    env.assert_rejected(certify(2, None), |e| matches!(e, FundingError::Unproven)).await;
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_rejects_world_and_leg_mismatch() {
    let env = Env::new(Shape::STRICT).await;
    // 世界与收款腿互校：载体腿记到 main 世界在进入事务前就被拒绝。
    let mut crossed = certify(2, None);
    crossed.world = SettlementWorld::Main;
    assert!(matches!(env.store.test_certify_cod(crossed).await, Err(FundingError::Invalid)));
    // 存储层同样互校冻结事实：把 isekai 合同的载体义务按 main 原件腿认证，腿与世界自洽但与合同不符。
    let mut forged = certify(3, None);
    forged.world = SettlementWorld::Main;
    forged.receipt.leg = CodLeg::Original;
    forged.gross = 101;
    env.assert_rejected(forged, |e| matches!(e, FundingError::Conflict)).await;
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_concurrent_workers_fund_once() {
    let env = Env::new(Shape::STRICT).await;
    let (first, second) = tokio::join!(
        env.store.test_certify_cod(certify(2, None)),
        env.store.test_certify_cod(certify(3, None))
    );
    let (first, second) = (first.expect("第一个 Worker"), second.expect("第二个 Worker"));
    assert_eq!(first.receipt_source_id, second.receipt_source_id);
    assert_eq!(
        [first.replayed, second.replayed].iter().filter(|replayed| !**replayed).count(),
        1,
        "只有一个 Worker 真正写入"
    );
    let after = ledger(env.pool()).await;
    assert_eq!(
        (after.receipts, after.funded, after.fee_facts, after.issued),
        (1, 1, 1, GROSS),
        "只产生一份收入"
    );
}
