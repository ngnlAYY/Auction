//! 迁移 000112 的历史投影修复：只收口有确凿证据（命令账本确认、原付款未终结且无外部写入、
//! 依赖未开始）的成交撤销，复杂或缺证据的记录保持原样。

use auction_storage::postgres_migrate_to;
use sqlx::PgPool;
use uuid::Uuid;

use super::fixture::{Sale, SaleSpec, insert_reclaiming_sale};
use super::{dispositions, fulfillment};
use crate::support;

/// 模拟迁移前的 Admin 确认：只落定 resolution 并转入重列等待，不收口原付款履约；
/// `ledger` 为假时缺少命令账本证据。
async fn void_without_closure(pool: &PgPool, sale: &Sale, resolution: &str, ledger: bool) {
    sqlx::query(
        "UPDATE public.settlement_recoveries
         SET stage = 'relist_pending', resolution = $2, next_attempt_at = clock_timestamp(),
             state_version = state_version + 1, updated_at = clock_timestamp()
         WHERE id = $1",
    )
    .bind(sale.recovery_id)
    .bind(resolution)
    .execute(pool)
    .await
    .expect("模拟旧确认");
    if ledger {
        sqlx::query(
            "INSERT INTO public.settlement_recovery_commands
                (command_id, recovery_id, expected_state_version, action, request_digest,
                 result_code, result_payload)
             VALUES ($1, $2, $3, $4, decode(repeat('d1', 32), 'hex'), 'ok', '{}'::jsonb)",
        )
        .bind(Uuid::new_v4())
        .bind(sale.recovery_id)
        .bind(sale.state_version)
        .bind(resolution)
        .execute(pool)
        .await
        .expect("写入确认命令账本");
    }
}

async fn sale(pool: &PgPool, world: &'static str, sequence: i64) -> Sale {
    insert_reclaiming_sale(
        pool,
        SaleSpec {
            world,
            sequence,
            cod_return_count: if world == "main" { 3 } else { 1 },
            cod_return_limit: 3,
            blacklist_satisfied: world == "main",
        },
    )
    .await
}

#[tokio::test]
async fn postgres_migration_112_repairs_only_provable_voided_payments() {
    let database = support::TestDatabase::create_empty().await.expect("新建空库");
    let store = database.maintenance_store().await.expect("以维护账号连接");
    let pool = store.postgres_pool();
    postgres_migrate_to(pool, 111).await.expect("迁移到 000111");

    let reclaimed = sale(pool, "main", 7_600_001).await;
    void_without_closure(pool, &reclaimed, "reclaimed", true).await;
    let cancelled = sale(pool, "isekai", 7_600_002).await;
    void_without_closure(pool, &cancelled, "cancelled", true).await;
    let unproven = sale(pool, "main", 7_600_003).await;
    void_without_closure(pool, &unproven, "reclaimed", false).await;
    let in_flight = sale(pool, "main", 7_600_004).await;
    void_without_closure(pool, &in_flight, "reclaimed", true).await;
    sqlx::query(
        "UPDATE public.deliveries SET status = 'pending', completed_at = NULL,
             external_reference = NULL
         WHERE id = $1",
    )
    .bind(in_flight.payment_delivery)
    .execute(pool)
    .await
    .expect("模拟仍在发送队列的原付款投递");
    let started = sale(pool, "main", 7_600_005).await;
    void_without_closure(pool, &started, "reclaimed", true).await;
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'manual_reconcile', last_error_code = 'evidence_mismatch',
             completed_at = clock_timestamp()
         WHERE obligation_id = $1",
    )
    .bind(started.seller)
    .execute(pool)
    .await
    .expect("模拟依赖履约已有结论");

    postgres_migrate_to(pool, 112).await.expect("执行 000112");

    assert_eq!(
        fulfillment(pool, reclaimed.payment).await,
        ("failed".to_owned(), Some("recovery_reclaimed".to_owned()))
    );
    assert_eq!(
        fulfillment(pool, reclaimed.seller).await,
        ("failed".to_owned(), Some("recovery_sale_voided".to_owned()))
    );
    assert_eq!(fulfillment(pool, reclaimed.refund).await, ("pending".to_owned(), None));
    assert_eq!(
        dispositions(pool, reclaimed.payment_delivery).await,
        vec![(
            "cod_returned".to_owned(),
            Some("recovery_reclaimed".to_owned()),
            "system".to_owned()
        )]
    );

    assert_eq!(
        fulfillment(pool, cancelled.payment).await,
        ("failed".to_owned(), Some("recovery_cancelled".to_owned()))
    );
    for dependent in [cancelled.handoff.expect("isekai 交付义务"), cancelled.seller] {
        assert_eq!(
            fulfillment(pool, dependent).await,
            ("failed".to_owned(), Some("recovery_sale_voided".to_owned()))
        );
    }

    // 缺命令账本、原付款投递仍在队列、依赖履约已有结论：都保持原样留给人工核对。
    for kept in [&unproven, &in_flight, &started] {
        assert_eq!(fulfillment(pool, kept.payment).await.0, "enqueued");
        assert!(dispositions(pool, kept.payment_delivery).await.is_empty());
    }
    assert_eq!(fulfillment(pool, unproven.seller).await.0, "pending");
    assert_eq!(fulfillment(pool, started.seller).await.0, "manual_reconcile");

    let summary: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM public.audit_records
         WHERE action = 'system.migration' AND entity_id = '000112'",
    )
    .fetch_one(pool)
    .await
    .expect("读取迁移审计");
    assert_eq!(summary["repaired_recoveries"], 2);
    assert_eq!(summary["closed_payments"], 2);
    assert_eq!(summary["closed_dependents"], 3);
    assert_eq!(summary["dispositions"], 2);
    assert_eq!(summary["closure_audits"], 5);

    store.postgres_pool().close().await;
}
