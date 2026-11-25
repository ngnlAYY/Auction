//! 第 7 期恢复写口的审计失败：退回处置与灭失确认整笔回滚；撤掉注入后同一命令恰提交一次。
//!
//! 保护不变量 3（处置 / 灭失事实、处分权变化与审计同事务）、8（审计只追加，失败不留半条事实）。

use std::sync::Arc;

use auction_application::consignment::{
    AutoRelistSetting, PositionLossStore, ReturnDispositionService, ReturnDispositionStore,
};

use super::super::returns::fixture::{Rts, seed_consumed};
use super::confirm_loss::fixture::{confirm, record_proof};
use super::sale_recovery::World;
use crate::consignment::audit_fault::{accept_audits, audit_rows, facts, reject_audits};
use crate::support::TestDatabase;

const SETTING_OFF: AutoRelistSetting = AutoRelistSetting { enabled: false, config_digest: [9; 32] };

#[tokio::test]
async fn postgres_consignment_recovery_resolve_return_audit_failure_rolls_back_the_disposition() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_consumed(pool, &store, Rts { n: 1 }).await;
    let port: Arc<dyn ReturnDispositionStore> = store.clone();
    let service = ReturnDispositionService::new(port);
    let pending = service.pending(10).await.expect("待处置");
    assert_eq!(pending.len(), 1);
    let before = facts(pool).await;
    let audits = audit_rows(pool).await;
    reject_audits(pool).await;

    let failed = service.resolve(pending[0], SETTING_OFF).await;
    assert!(failed.is_err(), "审计失败时处置必须失败");
    assert_eq!(facts(pool).await, before, "零处置、零回执，资金与处分权不变");
    assert_eq!(audit_rows(pool).await, audits);

    accept_audits(pool).await;
    let resolved = service.resolve(pending[0], SETTING_OFF).await.expect("撤掉注入后处置");
    assert!(!resolved.replayed, "回滚的那次没有留下可重放的处置");
    assert_eq!(facts(pool).await.commands, before.commands + 1, "恰一条处置回执");
    assert!(audit_rows(pool).await > audits);
}

#[tokio::test]
async fn postgres_consignment_loss_confirm_audit_failure_keeps_the_position_open() {
    let world = World::taken().await;
    let proof_id = record_proof(&world, 1).await;
    let pool = &world.owner;
    let before = facts(pool).await;
    let audits = audit_rows(pool).await;
    reject_audits(pool).await;

    let failed = world.deposit.confirm_position_loss(confirm(1, 2, proof_id, 0x41)).await;
    assert!(failed.is_err(), "审计失败时灭失确认必须失败");
    assert_eq!(facts(pool).await, before, "position 仍在库、零损失事实与案件、零回执");
    assert_eq!(audit_rows(pool).await, audits);

    accept_audits(pool).await;
    world
        .deposit
        .confirm_position_loss(confirm(1, 2, proof_id, 0x41))
        .await
        .expect("撤掉注入后同命令确认灭失");
    let after = facts(pool).await;
    assert_eq!(after.commands, before.commands + 1, "恰一条确认回执");
    assert_ne!(after.custody, before.custody, "处分权随确认关闭");
    assert!(audit_rows(pool).await > audits);
}
