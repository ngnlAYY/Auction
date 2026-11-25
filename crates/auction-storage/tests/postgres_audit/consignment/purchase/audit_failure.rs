//! 购买写口的审计失败：合同图、首轮、position、挂单推进与回执整笔回滚，命令 ID 不被占用；撤掉注入后同命令提交。
//!
//! 保护不变量 3（业务事实与追加审计同事务）、1（失败不占命令 ID，原样重试得到唯一结果）。

use auction_application::consignment::{PurchaseOutcome, PurchaseStore};

use super::fixture::{BUYER, Fixture, purchase};
use crate::consignment::audit_fault::{accept_audits, audit_rows, facts, reject_audits};

#[tokio::test]
async fn postgres_consignment_purchase_audit_failure_rolls_back_the_whole_sale() {
    let fixture = Fixture::available().await;
    let pool = fixture.pool();
    let before = facts(pool).await;
    let audits = audit_rows(pool).await;
    reject_audits(pool).await;

    let failed = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await;
    assert!(failed.is_err(), "审计失败时购买必须失败：{failed:?}");
    assert_eq!(facts(pool).await, before, "外写、命令、处分权、负债、证据与全库内容都不变");
    assert_eq!(audit_rows(pool).await, audits);

    // 对照：撤掉注入后同一命令第一次就提交，证明上面的失败来自审计而非夹具前提。
    accept_audits(pool).await;
    let bought = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("同命令提交");
    assert!(matches!(bought, PurchaseOutcome::Purchased(_)), "{bought:?}");
    let after = facts(pool).await;
    assert_eq!(after.commands, before.commands + 1, "恰一条购买回执");
    assert_eq!(after.posts, before.posts, "购买只建首轮，不起写");
    assert_ne!(after.custody, before.custody, "成交占用处分代");
    assert!(audit_rows(pool).await > audits, "审计与成交同事务写入");
}
