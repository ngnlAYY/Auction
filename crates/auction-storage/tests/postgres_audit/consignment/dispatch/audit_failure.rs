//! 派发起写写口的审计失败：起写事实、冻结 Send 尝试与寄售尝试整笔回滚，不发许可；撤掉注入后恰得一次许可。
//!
//! 保护不变量 3（同事务）与 4（外写只在本地提交后执行：没有提交的起写就没有 POST 许可）。

use auction_application::consignment::WriteStart;
use auction_domain::consignment::dispatch::AttemptKey;
use uuid::Uuid;

use super::fixture::{PRICE, Seeded, binding, frozen, round_key};
use crate::consignment::audit_fault::{accept_audits, audit_rows, facts, reject_audits};

#[tokio::test]
async fn postgres_consignment_dispatch_write_start_audit_failure_issues_no_permit() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let token = Uuid::new_v4();
    seeded.claim(token, 1).await;
    let attempt = AttemptKey { delivery_id: seeded.delivery_id, fence: 1 };
    let service = seeded.service();
    service.start_attempt(round_key(), attempt, token, frozen(PRICE)).await.expect("登记尝试");
    let before = facts(pool).await;
    let audits = audit_rows(pool).await;
    reject_audits(pool).await;

    let failed = service.mark_write_started(attempt, token, Some(binding())).await;
    assert!(failed.is_err(), "审计失败时起写必须失败");
    assert_eq!(facts(pool).await, before, "零起写、零 Send 尝试：不存在可 POST 的持久事实");
    assert_eq!(audit_rows(pool).await, audits);

    accept_audits(pool).await;
    let started =
        service.mark_write_started(attempt, token, Some(binding())).await.expect("撤掉注入后起写");
    assert!(
        matches!(started, WriteStart::FirstCommitted { .. }),
        "回滚的起写没有消耗许可：撤掉注入后恰得一次"
    );
    let committed = facts(pool).await;
    assert_eq!(committed.posts, before.posts + 2, "一条冻结 Send 尝试、一条寄售尝试");
    assert!(audit_rows(pool).await > audits, "审计与起写同事务写入");
    // 同命令重放只是“已提交”，不再发许可，也不新增任何事实。
    let replay = service.mark_write_started(attempt, token, Some(binding())).await;
    assert!(matches!(replay, Ok(WriteStart::AlreadyCommitted(_))));
    assert_eq!(facts(pool).await, committed);
}
