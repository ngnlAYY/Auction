//! 成交撤销后 Admin 投递台不能重新激活原结算投递（设计 5a）的 PostgreSQL 18 用例。
//!
//! - 三种 resolution（`reclaimed`/`cancelled`/`equipment_recovered`）下，原付款、交付与卖方货款投递
//!   的释放、重试与对账重试都返回 `SettlementIneligible`，投递、履约与恢复记录一行不改；
//! - 恢复流程接管退件之后（已消费退件、持有收回证明，或系统退件已到达待消费），原付款与交付
//!   投递的重寄只归恢复流程，三种动作同样拒绝；恢复流程按 owner 重寄过、当前没有退件时照常重试；
//! - 撤销前已提交的同一重试命令在撤销后仍重放原结果，不产生新动作；
//! - 履约指针已换代的旧派发同样拒绝；`equipment_recovered` 之后的买方退款投递照常重试；
//! - 并发：重试与确认经两条真实连接在恢复行锁上排队；恢复流程已消费退件，两种提交顺序下重试
//!   都被拒绝、确认照常提交，确认撤销之后不会再有被复活的待发送投递；
//! - 锁序：确认与 Admin 判失败都先锁原付款投递、再锁履约，排在同一投递行锁上的确认不持有履约行，
//!   两者不形成锁环。

use std::time::Duration as StdDuration;

use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminGuardBlocker,
    DeliveryAdminPort,
};
use auction_application::recovery::{RecoveryCommandRepository, RecoveryConfirmationAction};
use auction_storage::DatabaseStore;
use sqlx::PgPool;
use uuid::Uuid;

use super::fixture::{Sale, SaleSpec, insert_reclaiming_sale};
use super::recovered::fixture::insert_recovered_sale;
use super::{confirmation, fulfillment};
use crate::support;

/// 以 Admin 身份对投递执行一条人工命令；摘要按已发布规则计算。
fn admin_command(
    delivery_id: Uuid,
    fencing_version: i64,
    action: DeliveryAdminAction,
) -> DeliveryAdminCommand {
    DeliveryAdminCommand {
        command_id: Uuid::new_v4(),
        delivery_id,
        expected_fencing_version: fencing_version,
        action,
        external_reference: None,
        failure_code: None,
        actor_id: "admin".to_owned(),
        request_id: None,
        request_hash: action.request_hash(delivery_id, fencing_version, None, None, "admin"),
    }
}

/// 把投递改到动作要求的起始状态（Release 为 claimed，重试为 failed，对账重试为
/// manual_reconcile）并推进 fence；返回新的 fence。只用于构造夹具。
async fn stage_delivery(pool: &PgPool, delivery_id: Uuid, action: DeliveryAdminAction) -> i64 {
    let (status, leased) = match action {
        DeliveryAdminAction::Release => ("claimed", true),
        DeliveryAdminAction::RetryFailed => ("failed", false),
        _ => ("manual_reconcile", false),
    };
    sqlx::query_scalar(
        "UPDATE public.deliveries
         SET status = $2,
             lease_token = CASE WHEN $3 THEN gen_random_uuid() END,
             lease_expires_at = CASE WHEN $3 THEN clock_timestamp() + interval '1 hour' END,
             claimed_at = CASE WHEN $3 THEN clock_timestamp()
                               ELSE COALESCE(claimed_at, created_at) END,
             sending_at = CASE WHEN $3 THEN NULL
                               ELSE COALESCE(sending_at, claimed_at, created_at) END,
             completed_at = CASE WHEN $3 THEN NULL ELSE clock_timestamp() END,
             failure_code = CASE $2 WHEN 'failed' THEN 'send_rejected'
                                    WHEN 'manual_reconcile' THEN 'outcome_unknown' END,
             external_reference = NULL,
             attempt_count = GREATEST(attempt_count, 1),
             fencing_version = fencing_version + 1,
             updated_at = clock_timestamp()
         WHERE id = $1
         RETURNING fencing_version",
    )
    .bind(delivery_id)
    .bind(status)
    .bind(leased)
    .fetch_one(pool)
    .await
    .expect("构造投递起始状态")
}

/// 给义务写一条已寄出的结算投递并把其履约指向它；`state` 是履约状态。返回投递 ID。
async fn attach_delivery(pool: &PgPool, obligation: Uuid, state: &str) -> Uuid {
    let delivery_id = Uuid::new_v4();
    let prepare = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference)
         VALUES ($1, $2, decode(repeat('c4', 32), 'hex'), 'settlement', $3, 'Recovery buyer',
                 '{\"schema_version\":1,\"operation\":\"send\",\"world\":\"main\"}'::jsonb,
                 'succeeded', 1, 1, clock_timestamp(), clock_timestamp(), clock_timestamp(), $4)",
    )
    .bind(delivery_id)
    .bind(prepare)
    .bind(obligation.to_string())
    .bind(format!("mooglemail:main:send:{}", 660_000_000 + delivery_id.as_u128() % 1_000_000))
    .execute(pool)
    .await
    .expect("写入结算投递");
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = $2, delivery_id = $3, prepare_command_id = $4,
             prepare_request_digest = decode(repeat('c4', 32), 'hex'),
             completed_at = CASE WHEN $2 IN ('failed', 'manual_reconcile')
                                 THEN clock_timestamp() END,
             dormant = false, updated_at = clock_timestamp()
         WHERE obligation_id = $1",
    )
    .bind(obligation)
    .bind(state)
    .bind(delivery_id)
    .bind(prepare)
    .execute(pool)
    .await
    .expect("履约指向结算投递");
    delivery_id
}

async fn delivery_state(pool: &PgPool, delivery_id: Uuid) -> (String, i64) {
    sqlx::query_as("SELECT status, fencing_version FROM public.deliveries WHERE id = $1")
        .bind(delivery_id)
        .fetch_one(pool)
        .await
        .expect("读取投递状态")
}

async fn recovery_resolution(pool: &PgPool, recovery_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT resolution FROM public.settlement_recoveries WHERE id = $1")
        .bind(recovery_id)
        .fetch_one(pool)
        .await
        .expect("读取恢复记录")
}

fn ineligible(error: &ApplicationError) -> bool {
    matches!(error, ApplicationError::DeliveryAdmin(DeliveryAdminError::SettlementIneligible))
}

/// 三种动作逐一断言：返回 `SettlementIneligible`，投递状态与 fence、履约状态都不变。
async fn assert_reactivation_rejected(store: &DatabaseStore, delivery_id: Uuid, obligation: Uuid) {
    let pool = store.postgres_pool();
    for action in [
        DeliveryAdminAction::Release,
        DeliveryAdminAction::RetryFailed,
        DeliveryAdminAction::ReconcileRetry,
    ] {
        let fence = stage_delivery(pool, delivery_id, action).await;
        let before = (delivery_state(pool, delivery_id).await, fulfillment(pool, obligation).await);
        let error = store
            .execute(admin_command(delivery_id, fence, action))
            .await
            .expect_err("成交撤销后不能重新激活原结算投递");
        assert!(ineligible(&error), "{action:?}: {error:?}");
        let after = (delivery_state(pool, delivery_id).await, fulfillment(pool, obligation).await);
        assert_eq!(after, before, "{action:?} 被拒绝时不能写任何状态");
        let commands: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM public.delivery_admin_commands WHERE delivery_id = $1",
        )
        .bind(delivery_id)
        .fetch_one(pool)
        .await
        .expect("读取命令账本");
        assert_eq!(commands, 0, "{action:?} 被拒绝时不写命令账本");
    }
}

/// 模拟恢复流程已按 owner 重寄原 COD：当前退件指针与收回证明清空、回到等待付款，旧退件只剩
/// 追加的消费事实。
async fn clear_current_return(pool: &PgPool, recovery_id: Uuid) {
    sqlx::query(
        "UPDATE public.settlement_recoveries
         SET stage = 'awaiting_payment', return_mail_id = NULL, return_external_mail_id = NULL,
             custody_proof_kind = NULL, custody_proof_reference = NULL,
             custody_proof_digest = NULL, custody_proof_observed_at = NULL
         WHERE id = $1",
    )
    .bind(recovery_id)
    .execute(pool)
    .await
    .expect("清空当前退件指针");
}

async fn sale(store: &DatabaseStore, world: &'static str, sequence: i64) -> Sale {
    insert_reclaiming_sale(
        store.postgres_pool(),
        SaleSpec {
            world,
            sequence,
            cod_return_count: 3,
            cod_return_limit: 3,
            blacklist_satisfied: true,
        },
    )
    .await
}

#[tokio::test]
async fn postgres_admin_retry_projection_matches_recovery_owner_at_same_fence() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let parent = sale(&store, "main", 7_600_108).await;
    let delivery_id = Uuid::new_v4();
    let prepare = Uuid::new_v4();
    let activate = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             activation_command_id, activation_request_digest, activated_at)
         VALUES ($1, $2, decode(repeat('4b',32),'hex'), 'settlement_recovery_action', $3, 'Recovery buyer',
                 '{\"schema_version\":1,\"operation\":\"send\",\"world\":\"main\",\"template\":\"recovery_payment_reminder\"}'::jsonb,
                 'pending', $4, decode(repeat('4c',32),'hex'), clock_timestamp())",
    )
    .bind(delivery_id).bind(prepare).bind(format!("recovery:{}", parent.recovery_id)).bind(activate)
    .execute(pool).await.expect("写入恢复提醒投递");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_actions
            (id, recovery_id, kind, ordinal, execution_state, delivery_id, prepare_command_id,
             prepare_request_digest, activate_command_id, activate_request_digest, last_error_code, completed_at)
         VALUES (gen_random_uuid(), $1, 'payment_reminder', 3, 'failed', $2, $3,
                 decode(repeat('4b',32),'hex'), $4, decode(repeat('4c',32),'hex'),
                 'recovery_delivery_failed', clock_timestamp())",
    )
    .bind(parent.recovery_id).bind(delivery_id).bind(prepare).bind(activate)
    .execute(pool).await.expect("写入失败提醒动作");
    let action = DeliveryAdminAction::RetryFailed;
    let fence = stage_delivery(pool, delivery_id, action).await;

    for parent_blocker in ["recovery_delivery_conflict", "recovery_delivery_failed"] {
        sqlx::query("UPDATE public.settlement_recoveries SET blocking_code = $2, manual_reconcile_delivery_id = NULL WHERE id = $1")
            .bind(parent.recovery_id).bind(parent_blocker).execute(pool).await.expect("设置父恢复阻断");
        if parent_blocker == "recovery_delivery_failed" {
            sqlx::query("UPDATE public.settlement_recovery_actions SET execution_state = 'failed', last_error_code = 'fixture_failed' WHERE recovery_id = $1 AND kind = 'buyer_blacklist'")
                .bind(parent.recovery_id).execute(pool).await.expect("保留另一动作的失败阻断");
        }
        let (detail, guard, _) = store.detail_with_guard(delivery_id).await.expect("读取阻断投影");
        assert_eq!(detail.delivery.fencing_version, fence);
        let blocker = guard.projected_blocker(action).expect("恢复 owner 拒绝重排");
        assert_eq!(blocker, DeliveryAdminGuardBlocker::RecoveryRetryIneligible);
        let error = store
            .execute(admin_command(delivery_id, fence, action))
            .await
            .expect_err("实际提交应按同一谓词拒绝");
        assert!(
            matches!(error, ApplicationError::DeliveryAdmin(error) if error == blocker.error())
        );
        assert_eq!(delivery_state(pool, delivery_id).await, ("failed".to_owned(), fence));
    }

    sqlx::query("UPDATE public.settlement_recovery_actions SET execution_state = 'satisfied', last_error_code = NULL WHERE recovery_id = $1 AND kind = 'buyer_blacklist'")
        .bind(parent.recovery_id).execute(pool).await.expect("其它动作已完成");
    let (_, guard, _) = store.detail_with_guard(delivery_id).await.expect("读取准入投影");
    assert_eq!(guard.projected_blocker(action), None);
    let result = store
        .execute(admin_command(delivery_id, fence, action))
        .await
        .expect("实际提交应重排同一投递");
    assert_eq!(result.snapshot.status, "pending");
    assert_eq!(result.snapshot.fencing_version, fence + 1);
    let state: (String, Option<String>) = sqlx::query_as(
        "SELECT action.execution_state, recovery.blocking_code
         FROM public.settlement_recovery_actions action JOIN public.settlement_recoveries recovery ON recovery.id = action.recovery_id
         WHERE action.delivery_id = $1",
    ).bind(delivery_id).fetch_one(pool).await.expect("读取同事务恢复投影");
    assert_eq!(state, ("enqueued".to_owned(), None));
}

#[tokio::test]
async fn postgres_admin_delivery_actions_follow_fulfillment_state_at_the_same_fence() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    for (sequence, action, owner_state, next_state) in [
        (7_600_109, DeliveryAdminAction::RetryFailed, "failed", "enqueued"),
        (7_600_110, DeliveryAdminAction::ReconcileRetry, "manual_reconcile", "enqueued"),
        (7_600_111, DeliveryAdminAction::ReconcileFailed, "manual_reconcile", "failed"),
    ] {
        let sale = sale(&store, "main", sequence).await;
        clear_current_return(pool, sale.recovery_id).await;
        let delivery_id = sale.payment_delivery;
        let fence = stage_delivery(pool, delivery_id, action).await;
        let failure_code =
            (action == DeliveryAdminAction::ReconcileFailed).then_some("verified_not_sent");
        let command = DeliveryAdminCommand {
            failure_code: failure_code.map(str::to_owned),
            request_hash: action.request_hash(delivery_id, fence, None, failure_code, "admin"),
            ..admin_command(delivery_id, fence, action)
        };
        sqlx::query(
            "UPDATE public.settlement_obligation_fulfillments
             SET execution_state='enqueued', completed_at=NULL, last_error_code=NULL,
                 proof_kind=NULL, proof_reference=NULL, proof_digest=NULL, proof_observed_at=NULL
             WHERE obligation_id=$1",
        )
        .bind(sale.payment)
        .execute(pool)
        .await
        .expect("履约循环尚未收口投递结果");
        let (before, guard, _) = store.detail_with_guard(delivery_id).await.unwrap();
        assert_eq!(before.delivery.fencing_version, fence);
        assert_eq!(
            guard.projected_blocker(action),
            Some(DeliveryAdminGuardBlocker::SettlementFulfillmentState)
        );
        assert!(matches!(
            store.execute(command.clone()).await,
            Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::Stale))
        ));
        assert_eq!(store.detail(delivery_id).await.unwrap(), before);
        assert_eq!(fulfillment(pool, sale.payment).await.0, "enqueued");

        sqlx::query(
            "UPDATE public.settlement_obligation_fulfillments
             SET execution_state=$2, last_error_code='send_result', completed_at=clock_timestamp()
             WHERE obligation_id=$1",
        )
        .bind(sale.payment)
        .bind(owner_state)
        .execute(pool)
        .await
        .expect("履约循环已收口同一投递的结果");
        let (detail, guard, _) = store.detail_with_guard(delivery_id).await.unwrap();
        assert_eq!(detail.delivery.fencing_version, fence);
        assert_eq!(guard.projected_blocker(action), None);
        let result = store.execute(command).await.expect("同一 fence 现可执行");
        assert_eq!(result.snapshot.status, action.transition().next_status);
        assert_eq!(result.snapshot.fencing_version, fence + 1);
        assert_eq!(fulfillment(pool, sale.payment).await.0, next_state);
    }
}

/// 三种 resolution 下释放、重试与对账重试都拒绝重新激活原结算投递；未撤销的投递照常重试，
/// 撤销前已提交的命令重放原结果，旧派发拒绝，`equipment_recovered` 后的退款投递照常重试。
#[tokio::test]
async fn postgres_admin_retry_rejects_voided_settlement() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    // reclaimed（main）与 cancelled（isekai）：真实确认事务收口后的原付款投递。
    for (world, sequence, action) in [
        ("main", 7_600_001, RecoveryConfirmationAction::Reclaimed),
        ("isekai", 7_600_002, RecoveryConfirmationAction::Cancelled),
    ] {
        let voided = sale(&store, world, sequence).await;
        store.confirm_recovery(confirmation(&voided, action)).await.expect("确认成交撤销");
        assert_reactivation_rejected(&store, voided.payment_delivery, voided.payment).await;
    }

    // equipment_recovered：交付与卖方货款不再可执行；买方退款照常执行。
    let recovered = sale(&store, "isekai", 7_600_003).await;
    let handoff = recovered.handoff.expect("isekai 夹具带交付义务");
    let handoff_delivery = attach_delivery(pool, handoff, "failed").await;
    let seller_delivery = attach_delivery(pool, recovered.seller, "failed").await;
    let refund_delivery = attach_delivery(pool, recovered.refund, "failed").await;
    sqlx::query(
        "UPDATE public.settlement_recoveries
         SET stage = 'refunding', resolution = 'equipment_recovered',
             relisted_auction_id = auction_id, relisted_lot_id = lot_id
         WHERE id = $1",
    )
    .bind(recovered.recovery_id)
    .execute(pool)
    .await
    .expect("构造 equipment_recovered");
    assert_reactivation_rejected(&store, handoff_delivery, handoff).await;
    assert_reactivation_rejected(&store, seller_delivery, recovered.seller).await;
    let fence = stage_delivery(pool, refund_delivery, DeliveryAdminAction::RetryFailed).await;
    let retried = store
        .execute(admin_command(refund_delivery, fence, DeliveryAdminAction::RetryFailed))
        .await
        .expect("equipment_recovered 后的退款投递照常重试");
    assert_eq!(retried.snapshot.status, "pending");
    assert_eq!(fulfillment(pool, recovered.refund).await.0, "enqueued");

    // 未撤销正例：恢复流程已按 owner 重寄过、当前没有退件时，同一类原付款投递照常重试；撤销之后
    // 同一命令仍重放原结果，不产生新动作。
    let open = sale(&store, "main", 7_600_004).await;
    clear_current_return(pool, open.recovery_id).await;
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'failed', completed_at = clock_timestamp()
         WHERE obligation_id = $1",
    )
    .bind(open.payment)
    .execute(pool)
    .await
    .expect("构造失败的原付款履约");
    let fence = stage_delivery(pool, open.payment_delivery, DeliveryAdminAction::RetryFailed).await;
    let command = admin_command(open.payment_delivery, fence, DeliveryAdminAction::RetryFailed);
    let original = store.execute(command.clone()).await.expect("未撤销的投递照常重试");
    assert_eq!(original.snapshot.status, "pending");
    assert_eq!(fulfillment(pool, open.payment).await.0, "enqueued");
    sqlx::query(
        "UPDATE public.settlement_recoveries
         SET stage = 'relist_pending', resolution = 'reclaimed' WHERE id = $1",
    )
    .bind(open.recovery_id)
    .execute(pool)
    .await
    .expect("构造之后落定的成交撤销");
    let replayed = store.execute(command).await.expect("已提交命令先重放原结果");
    assert_eq!(replayed, original);
    assert_eq!(
        delivery_state(pool, open.payment_delivery).await,
        ("pending".to_owned(), fence + 1)
    );

    // 旧派发：履约已指向另一条投递，指向旧投递的重试即使义务未撤销也拒绝。
    let current = sale(&store, "main", 7_600_005).await;
    let stale_delivery = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count, claimed_at, sending_at, completed_at, failure_code)
         VALUES ($1, gen_random_uuid(), decode(repeat('c5', 32), 'hex'), 'settlement', $2,
                 'Recovery buyer', '{\"schema_version\":1,\"operation\":\"send\"}'::jsonb,
                 'failed', 1, 1, clock_timestamp(), clock_timestamp(), clock_timestamp(),
                 'send_rejected')",
    )
    .bind(stale_delivery)
    .bind(current.payment.to_string())
    .execute(pool)
    .await
    .expect("写入旧派发");
    let error = store
        .execute(admin_command(stale_delivery, 1, DeliveryAdminAction::RetryFailed))
        .await
        .expect_err("旧派发不能被重新激活");
    assert!(ineligible(&error), "{error:?}");
    assert_eq!(delivery_state(pool, stale_delivery).await, ("failed".to_owned(), 1));

    store.postgres_pool().close().await;
}

/// 等到本库中处于锁等待的会话数达到 `expected`；超时说明调用没有在恢复行锁上排队。
async fn wait_for_lock_waiters(pool: &PgPool, expected: i64) {
    for _ in 0..400 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(pool)
        .await
        .expect("读取锁等待");
        if waiting >= expected {
            return;
        }
        tokio::time::sleep(StdDuration::from_millis(25)).await;
    }
    panic!("没有等到 {expected} 个锁等待会话");
}

/// 两种提交顺序：第三条连接先锁住恢复行，让重试与确认依次排队，再放行。恢复流程已消费退件并
/// 持有收回证明，原付款投递的重寄只归恢复流程：无论谁先拿到锁，重试都被拒绝、确认照常提交，
/// 确认之后原投递保持人工核对状态与 fence，不会被复活。
#[tokio::test]
async fn postgres_admin_retry_races_recovery_confirmation() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    for (sequence, retry_first) in [(7_600_101, true), (7_600_102, false)] {
        let sale = sale(&store, "main", sequence).await;
        // 原付款投递结果未知、履约转人工：确认可以收口，对账重试也可以重新排队。
        sqlx::query(
            "UPDATE public.settlement_obligation_fulfillments
             SET execution_state = 'manual_reconcile', completed_at = clock_timestamp()
             WHERE obligation_id = $1",
        )
        .bind(sale.payment)
        .execute(pool)
        .await
        .expect("构造人工核对的原付款履约");
        let fence =
            stage_delivery(pool, sale.payment_delivery, DeliveryAdminAction::ReconcileRetry).await;
        let retry =
            admin_command(sale.payment_delivery, fence, DeliveryAdminAction::ReconcileRetry);
        let confirm = confirmation(&sale, RecoveryConfirmationAction::Reclaimed);

        let mut blocker = pool.begin().await.expect("开启阻塞事务");
        sqlx::query("SELECT id FROM public.settlement_recoveries WHERE id = $1 FOR UPDATE")
            .bind(sale.recovery_id)
            .execute(&mut *blocker)
            .await
            .expect("锁住恢复行");
        let (retry_task, confirm_task) = if retry_first {
            let retry_store = store.clone();
            let retry_task = tokio::spawn(async move { retry_store.execute(retry).await });
            wait_for_lock_waiters(pool, 1).await;
            let confirm_store = store.clone();
            let confirm_task =
                tokio::spawn(async move { confirm_store.confirm_recovery(confirm).await });
            wait_for_lock_waiters(pool, 2).await;
            (retry_task, confirm_task)
        } else {
            let confirm_store = store.clone();
            let confirm_task =
                tokio::spawn(async move { confirm_store.confirm_recovery(confirm).await });
            wait_for_lock_waiters(pool, 1).await;
            let retry_store = store.clone();
            let retry_task = tokio::spawn(async move { retry_store.execute(retry).await });
            wait_for_lock_waiters(pool, 2).await;
            (retry_task, confirm_task)
        };
        blocker.commit().await.expect("放行排队的两个事务");
        let retried = retry_task.await.expect("重试任务结束");
        let confirmed = confirm_task.await.expect("确认任务结束");

        let error = retried.expect_err("恢复流程已接管退件，重试在两种顺序下都必须拒绝");
        assert!(ineligible(&error), "retry_first={retry_first}: {error:?}");
        confirmed.expect("确认照常提交");
        assert_eq!(recovery_resolution(pool, sale.recovery_id).await.as_deref(), Some("reclaimed"));
        // 确认之后没有被复活的待发送投递：原投递保持人工核对状态与 fence。
        assert_eq!(
            delivery_state(pool, sale.payment_delivery).await,
            ("manual_reconcile".to_owned(), fence)
        );
        assert_eq!(
            fulfillment(pool, sale.payment).await,
            ("failed".to_owned(), Some("recovery_reclaimed".to_owned()))
        );
    }

    store.postgres_pool().close().await;
}

/// 恢复流程接管退件之后，原付款与交付投递的释放、重试与对账重试都拒绝：main COD 已消费退件并
/// 持有收回证明（未达退回上限，重寄归恢复流程）；isekai 交付的退件已被消费并已收回装备；系统
/// 退件已到达、尚待恢复流程消费。三者都不能把原投递改回待发送。
#[tokio::test]
async fn postgres_admin_retry_rejects_deliveries_whose_return_recovery_owns() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    let reclaiming = insert_reclaiming_sale(
        pool,
        SaleSpec {
            world: "main",
            sequence: 7_600_201,
            cod_return_count: 1,
            cod_return_limit: 3,
            blacklist_satisfied: false,
        },
    )
    .await;
    assert_reactivation_rejected(&store, reclaiming.payment_delivery, reclaiming.payment).await;

    let recovered = insert_recovered_sale(pool, 7_600_202).await;
    assert_reactivation_rejected(&store, recovered.handoff_delivery, recovered.handoff).await;

    // 系统退件已到达但恢复流程尚未消费：同样归恢复流程处理。
    let arriving = sale(&store, "main", 7_600_203).await;
    clear_current_return(pool, arriving.recovery_id).await;
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at, sender, subject)
         VALUES (gen_random_uuid(), 'recovery-confirm', '5560203', 'main', 'delivery',
                 decode(repeat('c6', 32), 'hex'), clock_timestamp() - interval '5 minutes',
                 'accepted', 'settlement_recovery_returned',
                 jsonb_build_object('obligation_id', $1::text, 'credits', 0, 'cod', 0,
                     'attachment_ids', '[]'::jsonb),
                 clock_timestamp(), 'MoogleMail', '[RTS] Auction equipment delivery [' || $1::text || ']')",
    )
    .bind(arriving.payment)
    .execute(pool)
    .await
    .expect("写入尚未消费的系统退件");
    assert_reactivation_rejected(&store, arriving.payment_delivery, arriving.payment).await;

    store.postgres_pool().close().await;
}

/// 确认与 Admin 判失败的锁序。原付款投递与履约都在人工核对、没有资金绑定时，判失败先锁投递再改写
/// 履约；确认若先锁履约再锁投递，两者会各持一把并互等。第三条连接先锁住原付款投递，让判失败与
/// 确认依次排队：此时确认不能已持有付款履约行（独立连接 `NOWAIT` 能立即锁到它），放行后判失败
/// 先提交，确认按 Serializable 冲突重试后照常收口，不出现死锁。
#[tokio::test]
async fn postgres_confirmation_and_reconcile_failed_lock_the_delivery_first() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = sale(&store, "main", 7_600_301).await;
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'manual_reconcile', completed_at = clock_timestamp()
         WHERE obligation_id = $1",
    )
    .bind(sale.payment)
    .execute(pool)
    .await
    .expect("构造人工核对的原付款履约");
    let fence =
        stage_delivery(pool, sale.payment_delivery, DeliveryAdminAction::ReconcileFailed).await;
    let action = DeliveryAdminAction::ReconcileFailed;
    let failed = DeliveryAdminCommand {
        failure_code: Some("verified_not_sent".to_owned()),
        request_hash: action.request_hash(
            sale.payment_delivery,
            fence,
            None,
            Some("verified_not_sent"),
            "admin",
        ),
        ..admin_command(sale.payment_delivery, fence, action)
    };
    let confirm = confirmation(&sale, RecoveryConfirmationAction::Reclaimed);

    let mut blocker = pool.begin().await.expect("开启阻塞事务");
    sqlx::query("SELECT id FROM public.deliveries WHERE id = $1 FOR UPDATE")
        .bind(sale.payment_delivery)
        .execute(&mut *blocker)
        .await
        .expect("锁住原付款投递");
    let failed_store = store.clone();
    let failed_task = tokio::spawn(async move { failed_store.execute(failed).await });
    wait_for_lock_waiters(pool, 1).await;
    let confirm_store = store.clone();
    let confirm_task = tokio::spawn(async move { confirm_store.confirm_recovery(confirm).await });
    wait_for_lock_waiters(pool, 2).await;

    // 两个事务都排在原付款投递上；确认此时不得已经持有付款履约行。
    let mut probe = pool.begin().await.expect("开启探测事务");
    sqlx::query(
        "SELECT obligation_id FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1 FOR UPDATE NOWAIT",
    )
    .bind(sale.payment)
    .execute(&mut *probe)
    .await
    .expect("排队中的确认不能先持有付款履约行，否则与判失败形成锁环");
    probe.rollback().await.expect("释放探测锁");

    blocker.commit().await.expect("放行排队的两个事务");
    let failed = failed_task.await.expect("判失败任务结束").expect("先排队的判失败提交");
    assert_eq!(failed.snapshot.status, "failed");
    confirm_task.await.expect("确认任务结束").expect("确认冲突重试后照常提交");
    assert_eq!(recovery_resolution(pool, sale.recovery_id).await.as_deref(), Some("reclaimed"));
    assert_eq!(
        fulfillment(pool, sale.payment).await,
        ("failed".to_owned(), Some("verified_not_sent".to_owned()))
    );
    assert_eq!(
        fulfillment(pool, sale.seller).await,
        ("failed".to_owned(), Some("recovery_sale_voided".to_owned()))
    );
    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT kind FROM public.delivery_dispositions WHERE delivery_id = $1 ORDER BY kind",
    )
    .bind(sale.payment_delivery)
    .fetch_all(pool)
    .await
    .expect("读取处置事实");
    assert_eq!(kinds, vec!["admin_failed".to_owned(), "cod_returned".to_owned()]);

    store.postgres_pool().close().await;
}
