//! 两个恢复重列入口：确认重列与到期自动重列的寄售强占用阻断及无占用对照。

use std::assert_matches;

use auction_application::auction::{AuctionLifecycleActor, AuctionLifecycleActorType};
use auction_application::core::ApplicationError;
use auction_application::recovery::{
    RecoveryCommandError, RecoveryCommandRepository, RecoveryConfirmationAction,
    RecoveryConfirmationCommand, RecoveryResolutionStore,
};
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_domain::{AuctionId, CommandId, SettlementId};
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::fixtures::{
    count_equipment_lots, count_lots, equipment_key, hold_equipment_lock,
    seed_confirmable_recovery, seed_draft_target, seed_position_claim, seed_reservation_claim,
    seed_scheduled_relist,
};
use crate::support::TestDatabase;

fn confirmation(recovery_id: Uuid, state_version: i64) -> RecoveryConfirmationCommand {
    let action = RecoveryConfirmationAction::Reclaimed;
    let reason = "";
    RecoveryConfirmationCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        recovery_id: SettlementId::from_uuid(recovery_id),
        expected_state_version: state_version,
        action,
        reason: reason.to_owned(),
        request_hash: RecoveryConfirmationCommand::legacy_request_hash(
            recovery_id,
            state_version,
            action,
            reason,
        )
        .expect("确认摘要"),
        request_id: None,
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Admin,
            actor_id: "admin".to_owned(),
        },
    }
}

async fn recovery_state(pool: &PgPool, recovery_id: Uuid) -> (String, Option<String>, i64) {
    sqlx::query_as(
        "SELECT stage, blocking_code, state_version
         FROM public.settlement_recoveries WHERE id=$1",
    )
    .bind(recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取恢复记录")
}

async fn recovery_command_count(pool: &PgPool, recovery_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_recovery_commands WHERE recovery_id=$1",
    )
    .bind(recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取恢复命令数")
}

async fn audit_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.audit_records")
        .fetch_one(pool)
        .await
        .expect("读取审计数")
}

fn equipment_42() -> ListingEquipmentIdentity {
    ListingEquipmentIdentity::new("main", "42").expect("固定身份")
}

#[tokio::test]
async fn postgres_confirm_entry_strong_claim_blocks_relist_without_new_lot() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let sale = seed_confirmable_recovery(pool, 20_000, "42").await;
    seed_reservation_claim(pool).await;
    let before = count_equipment_lots(pool, sale.sale.equipment_id).await;

    let command = confirmation(sale.recovery_id, 5);
    let result = store.confirm_recovery(command.clone()).await.expect("确认必须提交");
    assert_eq!(result.stage, "relist_pending");
    assert_eq!(result.resolution, "reclaimed");
    assert!(result.relisted_auction_id.is_none());
    assert!(result.relisted_lot_id.is_none());

    assert_eq!(
        recovery_state(pool, sale.recovery_id).await,
        ("relist_pending".to_owned(), Some("relist_equipment_held".to_owned()), 6)
    );
    assert_eq!(count_equipment_lots(pool, sale.sale.equipment_id).await, before, "不得新增 lot");
    let replayed = store.confirm_recovery(command).await.expect("重放确认");
    assert_eq!(replayed.state_version, result.state_version);
    assert_eq!(count_equipment_lots(pool, sale.sale.equipment_id).await, before);
}

#[tokio::test]
async fn postgres_scheduled_relist_strong_claim_blocks_relist_without_new_lot() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let (sale, recovery_id) = seed_scheduled_relist(pool, 20_100, "42").await;
    seed_reservation_claim(pool).await;
    let before = count_equipment_lots(pool, sale.equipment_id).await;

    let advance = store
        .advance_due_relist(Timestamp::now(), Duration::minutes(1))
        .await
        .expect("自动重列必须返回结果")
        .expect("取得待重列记录");
    assert_eq!(advance.recovery_id.as_uuid(), recovery_id);
    assert_eq!(advance.stage, "relist_pending");
    assert!(advance.relisted_auction_id.is_none());
    assert!(advance.relisted_lot_id.is_none());
    assert!(!advance.completed);
    assert_eq!(
        recovery_state(pool, recovery_id).await,
        ("relist_pending".to_owned(), Some("relist_equipment_held".to_owned()), 6)
    );
    assert_eq!(count_equipment_lots(pool, sale.equipment_id).await, before, "不得新增 lot");
}

#[tokio::test]
async fn postgres_scheduled_relist_position_claim_blocks_relist_without_new_lot() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let (sale, recovery_id) = seed_scheduled_relist(pool, 20_050, "42").await;
    let position = seed_position_claim(&database).await;
    let before = count_equipment_lots(pool, sale.equipment_id).await;

    let advance = store
        .advance_due_relist(Timestamp::now(), Duration::minutes(1))
        .await
        .expect("自动重列必须返回结果")
        .expect("取得待重列记录");
    assert_eq!(advance.recovery_id.as_uuid(), recovery_id);
    assert_eq!(advance.stage, "relist_pending");
    assert!(advance.relisted_auction_id.is_none());
    assert!(advance.relisted_lot_id.is_none());
    assert_eq!(recovery_state(pool, recovery_id).await.1.as_deref(), Some("relist_equipment_held"));
    assert_eq!(count_equipment_lots(pool, sale.equipment_id).await, before, "不得新增 lot");
    let holders: (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT reservation_id, position_id FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取强占用");
    assert_eq!(holders, (None, Some(position)));
}

#[tokio::test]
async fn postgres_confirm_entry_alias_claim_blocks_relist() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let sale = seed_confirmable_recovery(pool, 20_200, "00042").await;
    seed_reservation_claim(pool).await;
    let before = count_equipment_lots(pool, sale.sale.equipment_id).await;
    let result = store.confirm_recovery(confirmation(sale.recovery_id, 5)).await.expect("确认提交");
    assert_eq!(result.stage, "relist_pending");
    assert_eq!(
        recovery_state(pool, sale.recovery_id).await.1.as_deref(),
        Some("relist_equipment_held")
    );
    assert_eq!(count_equipment_lots(pool, sale.sale.equipment_id).await, before);
}

#[tokio::test]
async fn postgres_scheduled_relist_without_claim_still_relists() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let (sale, _recovery_id) = seed_scheduled_relist(pool, 20_300, "42").await;
    let draft = seed_draft_target(pool, "main", 20_301).await;
    let before = count_equipment_lots(pool, sale.equipment_id).await;
    let advance = store
        .advance_due_relist(Timestamp::now(), Duration::minutes(1))
        .await
        .expect("自动重列必须返回结果")
        .expect("取得待重列记录");
    assert_eq!(advance.relisted_auction_id, Some(draft));
    assert!(advance.relisted_lot_id.is_some());
    assert_eq!(count_equipment_lots(pool, sale.equipment_id).await, before + 1);
    assert_eq!(count_lots(pool, draft).await, 1);
}

#[tokio::test]
async fn postgres_scheduled_relist_unnormalizable_eid_keeps_old_behavior() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let (sale, _recovery_id) = seed_scheduled_relist(pool, 20_350, "000").await;
    let draft = seed_draft_target(pool, "main", 20_351).await;
    let before = count_equipment_lots(pool, sale.equipment_id).await;
    let advance = store
        .advance_due_relist(Timestamp::now(), Duration::minutes(1))
        .await
        .expect("自动重列必须返回结果")
        .expect("取得待重列记录");
    assert_eq!(advance.relisted_auction_id, Some(draft));
    assert!(advance.relisted_lot_id.is_some());
    assert_eq!(count_equipment_lots(pool, sale.equipment_id).await, before + 1);
    assert_eq!(count_lots(pool, draft).await, 1);
}

#[tokio::test]
async fn postgres_confirm_entry_without_claim_still_relists() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let sale = seed_confirmable_recovery(pool, 20_400, "42").await;
    let draft = seed_draft_target(pool, "main", 20_401).await;
    let before = count_equipment_lots(pool, sale.sale.equipment_id).await;
    let result = store
        .confirm_recovery(confirmation(sale.recovery_id, 5))
        .await
        .expect("无强占用时照常确认");
    assert_eq!(result.stage, "completed");
    assert_eq!(result.relisted_auction_id, Some(AuctionId::from_uuid(draft)));
    assert!(result.relisted_lot_id.is_some());
    assert_eq!(count_equipment_lots(pool, sale.sale.equipment_id).await, before + 1);
    assert_eq!(count_lots(pool, draft).await, 1);
}

#[tokio::test]
async fn postgres_confirm_entry_skips_unclaimed_equipment_lock_even_when_held() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let sale = seed_confirmable_recovery(pool, 22_500, "42").await;
    let draft = seed_draft_target(pool, "main", 22_501).await;
    let _holder = hold_equipment_lock(pool, equipment_key(&equipment_42())).await;
    let before = count_equipment_lots(pool, sale.sale.equipment_id).await;
    let result = store
        .confirm_recovery(confirmation(sale.recovery_id, 5))
        .await
        .expect("无寄售持有时不得取实物锁，外部会话锁不应阻塞确认");
    assert_eq!(result.stage, "completed");
    assert_eq!(result.relisted_auction_id, Some(AuctionId::from_uuid(draft)));
    assert_eq!(count_equipment_lots(pool, sale.sale.equipment_id).await, before + 1);
}

#[tokio::test]
async fn postgres_scheduled_relist_skips_unclaimed_equipment_lock_even_when_held() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let (sale, _recovery_id) = seed_scheduled_relist(pool, 22_510, "42").await;
    let draft = seed_draft_target(pool, "main", 22_511).await;
    let _holder = hold_equipment_lock(pool, equipment_key(&equipment_42())).await;
    let before = count_equipment_lots(pool, sale.equipment_id).await;
    let advance = store
        .advance_due_relist(Timestamp::now(), Duration::minutes(1))
        .await
        .expect("无寄售持有时不得取实物锁，外部会话锁不应阻塞自动重列")
        .expect("取得待重列记录");
    assert_eq!(advance.relisted_auction_id, Some(draft));
    assert_eq!(count_equipment_lots(pool, sale.equipment_id).await, before + 1);
}

/// 预读与锁内复核之间装备身份变化：必须按可重试冲突退出，且不写任何恢复事实。
#[tokio::test]
async fn postgres_confirm_entry_preload_mismatch_is_retryable_without_facts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let sale = seed_confirmable_recovery(pool, 22_600, "42").await;
    let before_lots = count_equipment_lots(pool, sale.sale.equipment_id).await;
    let before_commands = recovery_command_count(pool, sale.recovery_id).await;
    let before_audits = audit_count(pool).await;
    let equipment_id = sale.sale.equipment_id;
    let hook_pool = pool.clone();
    let result = store
        .test_confirm_postgres_once_with_between_hook(
            confirmation(sale.recovery_id, 5),
            Uuid::new_v4(),
            move || {
                let pool = hook_pool.clone();
                async move {
                    sqlx::query(
                        "UPDATE public.equipment SET external_equipment_id='43' WHERE id=$1",
                    )
                    .bind(equipment_id)
                    .execute(&pool)
                    .await
                    .expect("改变恢复记录指向的装备");
                }
            },
        )
        .await;
    assert_matches!(result, Err(RecoveryCommandError::SerializationConflict));
    assert_eq!(recovery_state(pool, sale.recovery_id).await, ("reclaiming".to_owned(), None, 5));
    assert_eq!(count_equipment_lots(pool, sale.sale.equipment_id).await, before_lots);
    assert_eq!(recovery_command_count(pool, sale.recovery_id).await, before_commands);
    assert_eq!(audit_count(pool).await, before_audits);
}

/// 自动重列的同一两段执行：身份变化必须是可重试冲突，不产生 lot、命令或审计。
#[tokio::test]
async fn postgres_scheduled_relist_preload_mismatch_is_retryable_without_facts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let (sale, recovery_id) = seed_scheduled_relist(pool, 22_700, "42").await;
    let before_lots = count_equipment_lots(pool, sale.equipment_id).await;
    let before_commands = recovery_command_count(pool, recovery_id).await;
    let before_audits = audit_count(pool).await;
    let equipment_id = sale.equipment_id;
    let hook_pool = pool.clone();
    let result = store
        .test_advance_relist_postgres_once_with_between_hook(
            Timestamp::now(),
            Duration::minutes(1),
            Uuid::new_v4(),
            move || {
                let pool = hook_pool.clone();
                async move {
                    sqlx::query(
                        "UPDATE public.equipment SET external_equipment_id='43' WHERE id=$1",
                    )
                    .bind(equipment_id)
                    .execute(&pool)
                    .await
                    .expect("改变恢复记录指向的装备");
                }
            },
        )
        .await;
    assert_matches!(result, Err(RecoveryCommandError::SerializationConflict));
    assert_eq!(recovery_state(pool, recovery_id).await, ("relist_pending".to_owned(), None, 5));
    assert_eq!(count_equipment_lots(pool, sale.equipment_id).await, before_lots);
    assert_eq!(recovery_command_count(pool, recovery_id).await, before_commands);
    assert_eq!(audit_count(pool).await, before_audits);
}

#[tokio::test]
async fn postgres_confirm_entry_waits_for_held_equipment_lock_without_facts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let sale = seed_confirmable_recovery(pool, 22_800, "42").await;
    seed_reservation_claim(pool).await;
    let _holder = hold_equipment_lock(pool, equipment_key(&equipment_42())).await;
    let before_lots = count_equipment_lots(pool, sale.sale.equipment_id).await;
    let before_commands = recovery_command_count(pool, sale.recovery_id).await;
    let before_audits = audit_count(pool).await;
    let result = store.confirm_recovery(confirmation(sale.recovery_id, 5)).await;
    match result {
        Err(ApplicationError::RecoveryCommand(RecoveryCommandError::Failed(message))) => {
            assert!(message.contains("实物锁暂不可用"), "{message}");
        }
        other => panic!("寄售持有且实物锁被占用时必须暂时不可用: {other:?}"),
    }
    assert_eq!(recovery_state(pool, sale.recovery_id).await, ("reclaiming".to_owned(), None, 5));
    assert_eq!(count_equipment_lots(pool, sale.sale.equipment_id).await, before_lots);
    assert_eq!(recovery_command_count(pool, sale.recovery_id).await, before_commands);
    assert_eq!(audit_count(pool).await, before_audits);
}

#[tokio::test]
async fn postgres_scheduled_relist_waits_for_held_equipment_lock_without_facts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let (sale, recovery_id) = seed_scheduled_relist(pool, 22_900, "42").await;
    seed_reservation_claim(pool).await;
    let _holder = hold_equipment_lock(pool, equipment_key(&equipment_42())).await;
    let before_lots = count_equipment_lots(pool, sale.equipment_id).await;
    let before_commands = recovery_command_count(pool, recovery_id).await;
    let before_audits = audit_count(pool).await;
    let result = store.advance_due_relist(Timestamp::now(), Duration::minutes(1)).await;
    match result {
        Err(ApplicationError::RecoveryCommand(RecoveryCommandError::Failed(message))) => {
            assert!(message.contains("实物锁暂不可用"), "{message}");
        }
        other => panic!("寄售持有且实物锁被占用时必须暂时不可用: {other:?}"),
    }
    assert_eq!(recovery_state(pool, recovery_id).await, ("relist_pending".to_owned(), None, 5));
    assert_eq!(count_equipment_lots(pool, sale.equipment_id).await, before_lots);
    assert_eq!(recovery_command_count(pool, recovery_id).await, before_commands);
    assert_eq!(audit_count(pool).await, before_audits);
}
