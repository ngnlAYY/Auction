//! 寄售专用领取：只领 ready 且前置 satisfied 的寄售义务；通用领取零寄售行；并发只有一个领取者；
//! 来源损坏转人工、不发（不变量 2、5、6）。

use std::time::Duration;

use auction_application::consignment::{
    ConsignmentFulfillmentClaims, ConsignmentGraphStore, ConsignmentObligationClaim,
};
use auction_application::settlement::SettlementFulfillmentStore;
use auction_domain::consignment::ConsignmentContractKind;
use auction_domain::{SettlementObligationKind, SettlementWorld};
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::{execute, succeeds};
use super::fixture::seed_main_custody;
use super::graph_write::{graph_rows, sale_graph, seed_funding};
use crate::support::TestDatabase;

/// 写入 main Sale 图后的测试库：三条义务，付款腿 ready，卖款 blocked，退款 contingent。
struct Seeded {
    _database: TestDatabase,
    owner: DatabaseStore,
    runtime: DatabaseStore,
}

impl Seeded {
    async fn main_sale() -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let runtime = database.store().await.expect("以运行账号连接");
        seed_main_custody(owner.postgres_pool()).await;
        seed_funding(owner.postgres_pool()).await;
        runtime.write_graph(sale_graph(SettlementWorld::Main, 1000)).await.expect("写入合同图");
        assert_eq!(graph_rows(owner.postgres_pool()).await, (1, 1, 3, 3));
        Self { _database: database, owner, runtime }
    }

    fn pool(&self) -> &PgPool {
        self.owner.postgres_pool()
    }

    async fn claim(&self) -> Vec<ConsignmentObligationClaim> {
        claim(&self.runtime).await
    }
}

async fn claim(store: &DatabaseStore) -> Vec<ConsignmentObligationClaim> {
    let now = Timestamp::now();
    store
        .claim_consignment_obligations(
            now,
            "consignment-test-worker",
            now + auction_time::Duration::minutes(5),
            10,
        )
        .await
        .expect("寄售领取")
}

/// 图写入派生的真实义务 ID，按写入顺序：付款、卖款、退款。
async fn graph_obligation_ids(pool: &PgPool) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM public.settlement_obligations WHERE contract_id IS NOT NULL
         ORDER BY CASE obligation_kind WHEN 'winner_equipment_cod' THEN 1
                  WHEN 'seller_payout' THEN 2 ELSE 3 END",
    )
    .fetch_all(pool)
    .await
    .expect("读取义务 ID")
}

#[tokio::test]
async fn postgres_consignment_claim_takes_only_ready_obligation_with_satisfied_prerequisite() {
    let seeded = Seeded::main_sale().await;
    let ids = graph_obligation_ids(seeded.pool()).await;
    let (payment, payout, refund) = (ids[0], ids[1], ids[2]);

    // 正例：只领付款腿；卖款 blocked、退款 contingent 不被领取。
    let claims = seeded.claim().await;
    assert_eq!(claims.len(), 1);
    let first = &claims[0];
    assert_eq!(first.obligation.id.as_uuid(), payment);
    assert_eq!(first.obligation.kind, SettlementObligationKind::WinnerEquipmentCod);
    assert_eq!(first.contract.kind(), ConsignmentContractKind::Sale);
    assert_eq!(first.dispatch_world, SettlementWorld::Main);
    assert_eq!(first.claim_fencing, 1);
    assert_eq!(first.external_equipment_id, "42");
    let frozen_name: String = sqlx::query_scalar("SELECT name FROM public.consignment_snapshots")
        .fetch_one(seeded.pool())
        .await
        .expect("冻结名称");
    assert_eq!(first.equipment_name, frozen_name);

    // 反例：租约期内不重复领取。
    assert!(seeded.claim().await.is_empty());

    // 反例：卖款即使被标成 ready，前置付款腿未 satisfied 也不领取；退款始终不领。
    succeeds(
        seeded.pool(),
        &format!("UPDATE public.settlement_obligations SET business_state = 'ready' WHERE id = '{payout}';"),
    )
    .await;
    assert!(seeded.claim().await.is_empty());

    // 正例：前置 satisfied 后只领卖款腿，派发世界是 main。
    succeeds(
        seeded.pool(),
        &format!(
            "UPDATE public.settlement_obligation_fulfillments
             SET execution_state = 'satisfied', completed_at = transaction_timestamp(),
                 claim_token = NULL, claimed_at = NULL, claim_expires_at = NULL
             WHERE obligation_id = '{payment}';"
        ),
    )
    .await;
    let claims = seeded.claim().await;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].obligation.id.as_uuid(), payout);
    assert_eq!(claims[0].obligation.kind, SettlementObligationKind::SellerPayout);
    assert_eq!(claims[0].dispatch_world, SettlementWorld::Main);
    assert!(seeded.claim().await.is_empty());
    let refund_claimed: Option<Uuid> = sqlx::query_scalar(
        "SELECT claim_token FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(refund)
    .fetch_one(seeded.pool())
    .await
    .expect("退款履约行");
    assert_eq!(refund_claimed, None, "contingent 的退款不得被领取");
}

#[tokio::test]
async fn postgres_consignment_claim_does_not_lock_position_or_contract() {
    // R5-C8：领取只读合同与 position，不加锁；第 6 期最终事务持有 position 锁时领取不得阻塞或与之死锁。
    let seeded = Seeded::main_sale().await;
    let mut holder = seeded.pool().begin().await.expect("持锁事务");
    sqlx::query("SELECT id FROM public.consignment_custody_positions FOR UPDATE")
        .fetch_all(&mut *holder)
        .await
        .expect("持有 position 行锁");
    let claims = tokio::time::timeout(Duration::from_secs(5), seeded.claim())
        .await
        .expect("领取不得因 position 行锁而阻塞");
    assert_eq!(claims.len(), 1, "position 被他人锁定时领取仍正常返回");
    holder.rollback().await.expect("释放行锁");
}

/// 一条最小的拍卖义务：场次、装备、lot、命令与 ready 的 unsold_return。
async fn seed_auction_obligation(pool: &PgPool) -> Uuid {
    let obligation = Uuid::new_v4();
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.auctions
                (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
                 scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
             SELECT 'd0000000-0000-4000-8000-000000000001', 'main', 1, 'Fake Auction', 'settled',
                    '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                    '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                    '{policy}'::jsonb;
             INSERT INTO public.equipment (id, world_id, external_equipment_id, name, category,
                                           collected_at)
             VALUES ('d0000000-0000-4000-8000-000000000002', 'main', '90042', 'Fake Equipment',
                     'Weapon', '2026-01-01T00:00:00Z');
             INSERT INTO public.auction_commands (command_id, command_kind, aggregate_id,
                                                  request_hash, result_code)
             VALUES ('d0000000-0000-4000-8000-000000000003', 'fake_settlement',
                     'd0000000-0000-4000-8000-000000000001', decode(repeat('11', 32), 'hex'),
                     'accepted');
             INSERT INTO public.lots (id, auction_id, equipment_id, lot_number, status, start_price,
                                      category_key, category_number, created_at, updated_at)
             VALUES ('d0000000-0000-4000-8000-000000000004',
                     'd0000000-0000-4000-8000-000000000001',
                     'd0000000-0000-4000-8000-000000000002', 1, 'unsold', 50000, 'Weapon', 1,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, recipient_game_identity, amount, equipment_id)
             VALUES ('{obligation}', 'd0000000-0000-4000-8000-000000000001',
                     'd0000000-0000-4000-8000-000000000004', 'main',
                     'd0000000-0000-4000-8000-000000000003', 1, 'unsold_return', 'ready',
                     'FakeSeller', 0, 'd0000000-0000-4000-8000-000000000002');",
            policy = crate::support::fixtures::policy_snapshot(
                Timestamp::parse_from_rfc3339("2026-01-01T00:00:00Z").expect("时间"),
                50_000,
                30_000
            )
        ),
    )
    .await;
    obligation
}

#[tokio::test]
async fn postgres_consignment_and_generic_claims_never_cross_origins() {
    let seeded = Seeded::main_sale().await;
    let auction_obligation = seed_auction_obligation(seeded.pool()).await;
    let consignment_ids = graph_obligation_ids(seeded.pool()).await;

    // 通用领取：只可能领到拍卖义务，寄售行的履约行必须原封不动。
    let now = Timestamp::now();
    let generic = SettlementFulfillmentStore::claim_due(
        &seeded.runtime,
        "generic-worker".to_owned(),
        now,
        now + auction_time::Duration::minutes(5),
        10,
    )
    .await
    .expect("通用领取");
    assert!(
        generic.iter().all(|claim| !consignment_ids.contains(&claim.obligation_id.as_uuid())),
        "通用领取不得领到寄售义务"
    );
    let touched: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = ANY($1) AND (claim_token IS NOT NULL OR claim_fencing <> 0)",
    )
    .bind(&consignment_ids)
    .fetch_one(seeded.pool())
    .await
    .expect("寄售履约行");
    assert_eq!(touched, 0);

    // 寄售领取：只领寄售义务，拍卖义务的履约行（fence 与状态版本）不被碰。
    let auction_row = || async {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT claim_fencing, state_version
             FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
        )
        .bind(auction_obligation)
        .fetch_one(seeded.pool())
        .await
        .expect("拍卖履约行")
    };
    let before = auction_row().await;
    let consignment = seeded.claim().await;
    assert_eq!(consignment.len(), 1);
    assert!(consignment_ids.contains(&consignment[0].obligation.id.as_uuid()));
    assert_eq!(auction_row().await, before, "寄售领取不得碰拍卖义务的履约行");
}

#[tokio::test]
async fn postgres_consignment_claim_gives_one_obligation_to_one_claimer() {
    let seeded = Seeded::main_sale().await;
    let payment = graph_obligation_ids(seeded.pool()).await[0];

    // 他人持有义务行锁时，跳过而不是等待，也不领取。
    let mut holder = seeded.pool().begin().await.expect("持锁事务");
    sqlx::query("SELECT id FROM public.settlement_obligations WHERE id = $1 FOR UPDATE")
        .bind(payment)
        .fetch_one(&mut *holder)
        .await
        .expect("持有义务行锁");
    let skipped = tokio::time::timeout(Duration::from_secs(5), seeded.claim())
        .await
        .expect("被锁义务必须被跳过而不是阻塞");
    assert!(skipped.is_empty());
    holder.rollback().await.expect("释放行锁");

    // 并发两个领取者，同一义务只有一个拿到。
    let (left, right) = tokio::join!(seeded.claim(), seeded.claim());
    assert_eq!(left.len() + right.len(), 1);
    let fencing: i64 = sqlx::query_scalar(
        "SELECT claim_fencing FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(payment)
    .fetch_one(seeded.pool())
    .await
    .expect("fence");
    assert_eq!(fencing, 1, "同一义务只登记一次领取");
}

/// 破坏一个来源事实后领取：义务转人工、不返回、不再被领取。
async fn assert_isolated(corrupt: &str, expected_code: &str) {
    let seeded = Seeded::main_sale().await;
    let payment = graph_obligation_ids(seeded.pool()).await[0];
    execute(seeded.pool(), &corrupt.replace("{payment}", &payment.to_string()))
        .await
        .expect("制造损坏来源");

    assert!(seeded.claim().await.is_empty(), "损坏来源不得被领取：{expected_code}");
    let (state, code): (String, Option<String>) = sqlx::query_as(
        "SELECT execution_state, last_error_code
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(payment)
    .fetch_one(seeded.pool())
    .await
    .expect("履约行");
    assert_eq!(state, "manual_reconcile");
    assert_eq!(code.as_deref(), Some(expected_code));
    assert!(seeded.claim().await.is_empty(), "转人工后不再被领取");
    let deliveries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_obligation_fulfillments WHERE delivery_id IS NOT NULL",
    )
    .fetch_one(seeded.pool())
    .await
    .expect("履约投递引用");
    assert_eq!(deliveries, 0, "损坏来源不得产生投递");
}

#[tokio::test]
async fn postgres_consignment_claim_isolates_corrupt_sources() {
    // 合同缺失（先摘掉外键与触发器，再删合同与子项）。
    assert_isolated(
        "ALTER TABLE public.settlement_obligations
             DROP CONSTRAINT c5_obligation_contract_fk,
             DROP CONSTRAINT c5_obligation_equipment_fk,
             DROP CONSTRAINT c5_obligation_world_fk;
         ALTER TABLE public.consignment_sales DROP CONSTRAINT c5_sale_contract_fk;
         ALTER TABLE public.consignment_rights DROP CONSTRAINT c7_right_contract_fk;
         ALTER TABLE public.consignment_contracts DISABLE TRIGGER USER;
         ALTER TABLE public.consignment_sales DISABLE TRIGGER USER;
         DELETE FROM public.consignment_sales;
         DELETE FROM public.consignment_contracts;",
        "consignment_source_missing",
    )
    .await;
    // 履约派发世界与义务世界不符。
    assert_isolated(
        "UPDATE public.settlement_obligation_fulfillments SET dispatch_world = 'isekai'
         WHERE obligation_id = '{payment}';",
        "consignment_world_mismatch",
    )
    .await;
    // 收款人与合同重建值不符。
    assert_isolated(
        "UPDATE public.settlement_obligations SET recipient_user_id = (SELECT seller_user_id FROM public.consignment_contracts)
         WHERE id = '{payment}';",
        "consignment_recipient_mismatch",
    )
    .await;
    // 义务世界与合同世界不符（派发世界随之改成匹配，只剩合同对义务的比较）。
    assert_isolated(
        "ALTER TABLE public.settlement_obligations DROP CONSTRAINT c5_obligation_world_fk;
         ALTER TABLE public.settlement_obligations DROP CONSTRAINT settlement_obligations_kind_shape_check;
         ALTER TABLE public.settlement_obligations DISABLE TRIGGER USER;
         UPDATE public.settlement_obligations SET world_id = 'isekai' WHERE id = '{payment}';
         UPDATE public.settlement_obligation_fulfillments SET dispatch_world = 'isekai'
         WHERE obligation_id = '{payment}';",
        "consignment_world_mismatch",
    )
    .await;
    // position 世界与合同世界不符。
    assert_isolated(
        "ALTER TABLE public.consignment_contracts DROP CONSTRAINT c5_contract_position_fk;
         DO $$ DECLARE c text; BEGIN
             FOR c IN SELECT format('%s DROP CONSTRAINT %I', conrelid::regclass, conname)
                      FROM pg_constraint
                      WHERE contype = 'f'
                        AND 'public.consignment_custody_positions'::regclass IN (conrelid, confrelid)
             LOOP
                 EXECUTE 'ALTER TABLE ' || c;
             END LOOP;
         END $$;
         ALTER TABLE public.consignment_custody_positions DISABLE TRIGGER USER;
         UPDATE public.consignment_custody_positions SET world_id = 'isekai';",
        "consignment_world_mismatch",
    )
    .await;
    // 付款义务金额与 Sale 标价不符。
    assert_isolated(
        "UPDATE public.settlement_obligations SET amount = 999 WHERE id = '{payment}';",
        "consignment_amount_mismatch",
    )
    .await;
}
