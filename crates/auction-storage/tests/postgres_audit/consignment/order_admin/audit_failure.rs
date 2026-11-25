//! 订单 Admin resolve 写口的审计失败：退款处置、6B 资金转换、封闭事实与订单事件整笔回滚；撤掉注入后同命令恰提交一次。
//!
//! 保护不变量 3（处置、资金转换与审计同事务）与 1（失败不占命令 ID）。

use auction_application::consignment::{OrderResolveAction, OrderResolveReceipt};
use uuid::Uuid;

use super::super::contracts::fixture::CONTRACT;
use super::super::deposit::support::succeeds;
use super::super::recovery::sale_recovery::refund_call::funded_sale;
use super::super::recovery::{PROOF, command_ids, proof_sql};
use super::fixture::{resolve_input, service};
use crate::consignment::audit_fault::{accept_audits, audit_rows, facts, reject_audits};

#[tokio::test]
async fn postgres_consignment_order_resolve_audit_failure_rolls_back_the_refund_conversion() {
    let (database, pool, store) = funded_sale().await;
    drop(store);
    let commands = command_ids(&pool, 1).await;
    succeeds(&pool, &proof_sql(PROOF, "fake-observation-1", &commands[0])).await;
    let service = service(database.store().await.expect("运行账号"));
    let contract = Uuid::parse_str(CONTRACT).expect("合同");
    let proof = Uuid::parse_str(PROOF).expect("证据");
    let input = || resolve_input(1, contract, 1, OrderResolveAction::RefundLost, proof);
    let before = facts(&pool).await;
    let audits = audit_rows(&pool).await;
    reject_audits(&pool).await;

    let failed = service.resolve(input()).await;
    assert!(failed.is_err(), "审计失败时 resolve 必须失败");
    assert_eq!(facts(&pool).await, before, "零处置、零资金转换、零封闭、零回执");
    assert_eq!(audit_rows(&pool).await, audits);

    accept_audits(&pool).await;
    let receipt = service.resolve(input()).await.expect("撤掉注入后同命令提交");
    assert!(matches!(receipt, OrderResolveReceipt::ResolutionRecorded { .. }), "{receipt:?}");
    let after = facts(&pool).await;
    assert_eq!(after.commands, before.commands + 1, "恰一条 resolve 回执");
    assert_ne!(after.liabilities, before.liabilities, "退款经 6B 单写者转换");
    assert!(audit_rows(&pool).await > audits);
}
