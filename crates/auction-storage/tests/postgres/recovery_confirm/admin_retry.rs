//! 成交撤销后 Admin 投递台不能重新激活原结算投递（设计 5a）的 PostgreSQL 18 用例。
//!
//! - 三种 resolution（`reclaimed`/`cancelled`/`equipment_recovered`）下，原付款、交付与卖方货款投递
//!   的释放、重试与对账重试都返回 `SettlementIneligible`，投递、履约与恢复记录一行不改；
//! - 未撤销的同一类投递照常重试；撤销前已提交的同一重试命令在撤销后仍重放原结果，不产生新动作；
//! - 履约指针已换代的旧派发同样拒绝；`equipment_recovered` 之后的买方退款投递照常重试；
//! - 并发：重试与确认经两条真实连接在恢复行锁上排队，两种提交顺序都只有一边成功，确认撤销
//!   之后不会再有被复活的待发送投递。

use std::time::Duration as StdDuration;

use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminPort,
};
use auction_application::recovery::{
    RecoveryCommandError, RecoveryCommandRepository, RecoveryConfirmationAction,
};
use auction_storage::DatabaseStore;
use sqlx::PgPool;
use uuid::Uuid;

use super::fixture::{Sale, SaleSpec, insert_reclaiming_sale};
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
             updated_at = clock_timestamp()
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

    // 未撤销正例：同一类原付款投递照常重试；撤销之后同一命令仍重放原结果，不产生新动作。
    let open = sale(&store, "main", 7_600_004).await;
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

/// 两种提交顺序：第三条连接先锁住恢复行，让重试与确认依次排队，再放行；先拿到锁的一边提交，
/// 另一边按 Serializable 冲突重试后看到已提交的结果并拒绝不安全的动作。
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

        if retry_first {
            let retried = retried.expect("先拿到锁的重试提交");
            assert_eq!(retried.snapshot.status, "pending");
            let error = confirmed.expect_err("投递已重新排队，确认必须拒绝");
            assert!(
                matches!(
                    error,
                    ApplicationError::RecoveryCommand(RecoveryCommandError::NotConfirmable)
                ),
                "{error:?}"
            );
            assert_eq!(recovery_resolution(pool, sale.recovery_id).await, None);
            assert_eq!(fulfillment(pool, sale.payment).await.0, "enqueued");
        } else {
            confirmed.expect("先拿到锁的确认提交");
            let error = retried.expect_err("成交已撤销，重试必须拒绝");
            assert!(ineligible(&error), "{error:?}");
            assert_eq!(
                recovery_resolution(pool, sale.recovery_id).await.as_deref(),
                Some("reclaimed")
            );
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
    }

    store.postgres_pool().close().await;
}
