//! 成交撤销后资产对账按恢复去向计数（集成第 2 阶段决定）的 PostgreSQL 18 用例：全部收口都走真实
//! 确认或 Worker 事务，报告走生产入口并以运行账号执行。
//!
//! - main 收回、isekai 取消、三次退回与 `equipment_recovered` 的原投递当前 fence 上有恢复来源的
//!   权威处置：不再列为在途或在途待核对，装备按恢复去向计入应持（收回待重列、重列到草稿场次），
//!   退回卖家并被领取的不再出现，同一装备只计一次；
//! - 普通的寄出后失败（没有恢复收口）仍是在途待核对；
//! - 偿付只按精确收口事实移出已撤销的卖方货款，`equipment_recovered` 激活的买方退款照常计入。

use auction_application::reconciliation::{
    AssetArmoryObservation, AssetReconciliationStore, CustodyInTransitReason, CustodyMissingSample,
};
use auction_application::recovery::{
    RecoveryCommandRepository, RecoveryConfirmationAction, RecoveryResolutionStore,
};
use auction_domain::SettlementWorld;
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::confirmation;
use super::fixture::{Sale, SaleSpec, insert_reclaiming_sale};
use super::recovered::fixture::{insert_recovered_sale, insert_relist_target};
use crate::support;

fn observation(world: SettlementWorld) -> AssetArmoryObservation {
    AssetArmoryObservation {
        world,
        observed_at: Timestamp::now(),
        equipment_ids: Vec::new(),
        complete: true,
        failure_code: None,
    }
}

async fn external_equipment_id(pool: &PgPool, sale: &Sale) -> String {
    sqlx::query_scalar(
        "SELECT equipment.external_equipment_id
         FROM public.settlement_recoveries AS recovery
         JOIN public.equipment AS equipment ON equipment.id = recovery.equipment_id
         WHERE recovery.id = $1",
    )
    .bind(sale.recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取外部装备 ID")
}

async fn sale(pool: &PgPool, world: &'static str, sequence: i64) -> Sale {
    insert_reclaiming_sale(
        pool,
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
async fn postgres_reconciliation_counts_recovered_equipment_where_recovery_placed_it() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    // main 收回：确认后没有草稿目标，停在收回待重列。
    let reclaimed = sale(pool, "main", 7_900_001).await;
    store
        .confirm_recovery(confirmation(&reclaimed, RecoveryConfirmationAction::Reclaimed))
        .await
        .expect("确认 reclaimed");

    // 三次退回：Worker 把装备退回卖家，卖家随后领取了退回邮件。
    let returned = sale(pool, "main", 7_900_002).await;
    store.advance_due_cod_return(Timestamp::now()).await.expect("推进 COD 退回").expect("退回卖家");
    let withdrawn: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.settlement_obligations
         WHERE lot_id = (SELECT lot_id FROM public.settlement_recoveries WHERE id = $1)
           AND obligation_kind = 'withdrawn_return'",
    )
    .bind(returned.recovery_id)
    .fetch_one(pool)
    .await
    .expect("退回卖家义务");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, dispatch_world, delivery_operation, proof_kind,
             proof_reference, proof_digest, proof_observed_at, completed_at)
         VALUES ($1, 'satisfied', 'main', 'send', 'mooglemail_sent_collected', '7654321',
                 decode(repeat('f1', 32), 'hex'), clock_timestamp(), clock_timestamp())",
    )
    .bind(withdrawn)
    .execute(pool)
    .await
    .expect("卖家领取退回装备");

    // 普通的寄出后失败：没有恢复收口，仍是在途待核对。
    let ordinary = sale(pool, "main", 7_900_003).await;
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'failed', last_error_code = 'evidence_mismatch',
             completed_at = clock_timestamp()
         WHERE obligation_id = $1",
    )
    .bind(ordinary.payment)
    .execute(pool)
    .await
    .expect("普通失败");

    // isekai 取消：交付从未寄出，确认后停在收回待重列。
    let cancelled = sale(pool, "isekai", 7_900_004).await;
    store
        .confirm_recovery(confirmation(&cancelled, RecoveryConfirmationAction::Cancelled))
        .await
        .expect("确认 cancelled");

    // equipment_recovered：已寄出的交付被收回，确认后重列到草稿场次。
    let recovered = insert_recovered_sale(pool, 7_900_005).await;
    insert_relist_target(pool, 7_900_005).await;
    store
        .confirm_recovery(confirmation(
            &recovered.sale,
            RecoveryConfirmationAction::EquipmentRecovered,
        ))
        .await
        .expect("确认 equipment_recovered");

    let custody = store
        .reconcile_custody(vec![
            observation(SettlementWorld::Isekai),
            observation(SettlementWorld::Main),
        ])
        .await
        .expect("生成装备持有报告");
    assert_eq!(custody.failure_code, None, "报告 SQL 必须在真实 schema 上执行成功");
    let world = |name: &str| {
        custody.worlds.iter().find(|world| world.world == name).expect("世界报告").clone()
    };

    let main = world("main");
    assert_eq!(
        main.missing_samples,
        [CustodyMissingSample {
            external_equipment_id: external_equipment_id(pool, &reclaimed).await,
            reason: "recovery_relist_pending".to_owned(),
        }]
    );
    assert_eq!((main.expected_count, main.in_transit_count), (1, 0));
    assert_eq!(
        main.in_transit_review_reasons,
        [CustodyInTransitReason {
            reason: "obligation_winner_equipment_cod_failed".to_owned(),
            count: 1,
        }],
        "只有没有恢复收口的普通失败仍是在途待核对"
    );
    assert_eq!(main.in_transit_review_count, 1);

    let isekai = world("isekai");
    let mut samples = isekai.missing_samples.clone();
    samples.sort_by(|left, right| left.reason.cmp(&right.reason));
    assert_eq!(
        samples,
        [
            CustodyMissingSample {
                external_equipment_id: external_equipment_id(pool, &cancelled).await,
                reason: "recovery_relist_pending".to_owned(),
            },
            CustodyMissingSample {
                external_equipment_id: external_equipment_id(pool, &recovered.sale).await,
                reason: "relisted_draft".to_owned(),
            },
        ]
    );
    assert_eq!(
        (isekai.expected_count, isekai.in_transit_count, isekai.in_transit_review_count),
        (2, 0, 0)
    );

    // 偿付：四条已撤销的卖方货款按精确收口移出，只剩普通成交的卖方货款；激活的退款照常计入。
    let solvency = store.report_solvency(3_600).await.expect("生成偿付报告");
    assert_eq!(solvency.failure_code, None);
    let main = solvency.worlds.iter().find(|world| world.world == "main").expect("main 偿付");
    assert_eq!((main.pending_seller_payout_count, main.pending_seller_payout_credits), (1, 100));
    assert_eq!((main.pending_refund_count, main.pending_refund_credits), (1, 100));
    assert_eq!(main.in_transit_review_count, 0);

    store.postgres_pool().close().await;
}
