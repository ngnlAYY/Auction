//! 退件消费写口的审计失败：证明、消费、资金回补与阻断解除整笔回滚；撤掉注入后同一消费恰提交一次。
//!
//! 保护不变量 3（证明、消费、回补与审计同事务）与 1（同输入重试得到唯一结果）。

use std::sync::Arc;

use super::fixture::{Rts, SIGNED_VALIDATOR, consume_input, seed_started_unproven, service};
use crate::consignment::audit_fault::{accept_audits, audit_rows, facts, reject_audits};
use crate::support::TestDatabase;

#[tokio::test]
async fn postgres_consignment_returns_consume_audit_failure_rolls_back_proof_and_cash_return() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_started_unproven(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);
    let input = consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7);
    let before = facts(pool).await;
    let audits = audit_rows(pool).await;
    reject_audits(pool).await;

    let failed = service.consume(input.clone()).await;
    assert!(failed.is_err(), "审计失败时消费必须失败");
    assert_eq!(facts(pool).await, before, "零证明、零消费、零回补，阻断仍在");
    assert_eq!(audit_rows(pool).await, audits);

    accept_audits(pool).await;
    let consumed = service.consume(input).await.expect("撤掉注入后同一消费提交");
    assert!(!consumed.replayed, "回滚的那次没有留下可重放的回执");
    let after = facts(pool).await;
    assert_eq!(after.evidence, before.evidence + 1, "恰一份退回证明");
    assert_ne!(after.liabilities, before.liabilities, "资金回补与阻断解除同事务落地");
    assert!(audit_rows(pool).await > audits);
}
