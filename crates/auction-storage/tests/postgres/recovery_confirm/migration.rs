//! 迁移 000112 的历史投影修复：只收口能证明确认、退件、Take 与被处置发送尝试彼此一致的成交
//! 撤销（命令账本确认、原付款已寄出等待领取且是唯一派发、系统退件已被消费并由对应 Take 取回、
//! 发送早于退件且确认后没有新 fence、依赖未开始），复杂或缺证据的记录保持原样。

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

    let inconsistent = inconsistent_cases(pool).await;

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

    // 无法证明确认、退件、Take 与被处置发送一致的记录同样保持原样（反例见 inconsistent_cases）。
    for (label, kept, payment_state) in &inconsistent {
        assert_eq!(fulfillment(pool, kept.payment).await.0, *payment_state, "{label}");
        assert!(dispositions(pool, kept.payment_delivery).await.is_empty(), "{label}");
        assert_eq!(fulfillment(pool, kept.seller).await.0, "pending", "{label}");
    }

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

/// 夹具语句绑定的成交身份。
#[derive(Clone, Copy)]
enum Bind {
    /// 原付款投递。
    Delivery,
    /// 原付款义务。
    Payment,
}

/// 一条夹具语句及其按顺序绑定的成交身份。
type Statement = (&'static str, &'static [Bind]);

/// 旧筛选条件会修复、新条件必须保持原样的历史状态：命令账本有确认、原付款仍未终结、依赖未开始，
/// 但确认、退件、Take 与被处置的发送尝试无法证明一致。返回（说明、成交、迁移后应保持的付款履约
/// 状态）。
async fn inconsistent_cases(pool: &PgPool) -> Vec<(&'static str, Sale, &'static str)> {
    let cases: [(&str, &[Statement], &str); 6] = [
        (
            "原付款投递写入未知、履约转人工核对",
            &[
                (
                    "UPDATE public.deliveries
                     SET status = 'manual_reconcile', failure_code = 'write_unknown',
                         external_reference = NULL,
                         result_metadata = '{\"operation\":\"send\",\"write_started\":true}'::jsonb
                     WHERE id = $1",
                    &[Bind::Delivery],
                ),
                (
                    "UPDATE public.settlement_obligation_fulfillments
                     SET execution_state = 'manual_reconcile', completed_at = clock_timestamp()
                     WHERE obligation_id = $1",
                    &[Bind::Payment],
                ),
            ],
            "manual_reconcile",
        ),
        (
            "确认之后原投递又有 Admin 命令（新 fence）",
            &[
                (
                    "INSERT INTO public.delivery_admin_commands
                        (command_id, command_kind, request_digest, delivery_id, result_payload)
                     VALUES (gen_random_uuid(), 'reconcile', decode(repeat('d3', 32), 'hex'), $1,
                             '{}'::jsonb)",
                    &[Bind::Delivery],
                ),
                (
                    "UPDATE public.deliveries SET fencing_version = fencing_version + 1
                     WHERE id = $1",
                    &[Bind::Delivery],
                ),
            ],
            "enqueued",
        ),
        (
            "退件之后原投递又有冻结发送尝试",
            &[(
                "INSERT INTO public.delivery_send_attempts
                    (delivery_id, fencing_version, command_id, request_digest, world_id,
                     sending_at, expectation)
                 SELECT id, fencing_version + 1, gen_random_uuid(),
                        decode(repeat('d4', 32), 'hex'), payload_world, clock_timestamp(),
                        jsonb_build_object('world_id', payload_world, 'recipient', recipient,
                            'subject', 'Recovery migration', 'body_digest',
                            to_jsonb(array_fill(7, ARRAY[32])), 'credits', 0, 'cod', 0,
                            'equipment_ids', '[]'::jsonb, 'credential', 'null'::jsonb)
                 FROM public.deliveries WHERE id = $1",
                &[Bind::Delivery],
            )],
            "enqueued",
        ),
        (
            "原付款义务另有派发",
            &[(
                "INSERT INTO public.deliveries
                    (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                     status, failure_code, completed_at)
                 VALUES (gen_random_uuid(), gen_random_uuid(), decode(repeat('d5', 32), 'hex'),
                         'settlement', $1::text, 'Recovery buyer',
                         '{\"schema_version\":1,\"operation\":\"send\",\"world\":\"main\"}'::jsonb,
                         'failed', 'send_rejected', clock_timestamp())",
                &[Bind::Payment],
            )],
            "enqueued",
        ),
        (
            "退件来源不可证明",
            &[(
                "UPDATE public.inbound_mail SET sender = NULL, subject = NULL
                 WHERE id = (SELECT return_mail_id FROM public.settlement_recoveries
                             WHERE payment_obligation_id = $1)",
                &[Bind::Payment],
            )],
            "enqueued",
        ),
        (
            "Take 证明与恢复记录的收回证明不符",
            &[(
                "UPDATE public.settlement_recovery_actions
                 SET proof_digest = decode(repeat('d6', 32), 'hex')
                 WHERE kind = 'reclaim_equipment_take'
                   AND recovery_id = (SELECT id FROM public.settlement_recoveries
                                      WHERE payment_obligation_id = $1)",
                &[Bind::Payment],
            )],
            "enqueued",
        ),
    ];
    let mut kept = Vec::with_capacity(cases.len());
    for (index, (label, statements, payment_state)) in cases.into_iter().enumerate() {
        let sequence = 7_600_101 + i64::try_from(index).expect("用例序号");
        let sale = sale(pool, "main", sequence).await;
        void_without_closure(pool, &sale, "reclaimed", true).await;
        for (statement, binds) in statements {
            let mut query = sqlx::query(sqlx::AssertSqlSafe(*statement));
            for bind in *binds {
                query = query.bind(match bind {
                    Bind::Delivery => sale.payment_delivery,
                    Bind::Payment => sale.payment,
                });
            }
            query.execute(pool).await.expect(label);
        }
        kept.push((label, sale, payment_state));
    }
    kept
}
