//! 结算出站投递的正文变量形状与资产冲突门的 PostgreSQL 18 用例。
//!
//! 冲突门扫描全库未满足的结算投递并按模板核对变量键集合，无法识别的形状无法证明与目标装备
//! 无关，按冲突失败关闭。回归：生成方给竞得装备 COD 与载体付款加入 `payment_deadline`、给
//! 载体付款、卖方货款与买方退款加入 `item_name` 后冲突门未同步，库里一有新形状投递，全部
//! 内部履约与装备损失都判冲突。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_storage::{DatabaseStore, asset_conflict_gate_passes};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;

/// 冲突门的目标装备；夹具投递引用的装备与它不同，只考察形状判定。
const TARGET_EID: &str = "424242";
/// 生成方 `format_deadline` 的 UTC 秒精度输出。
const DEADLINE: &str = "2026-11-22T12:19:28Z";

async fn gate_passes(store: &DatabaseStore) -> bool {
    asset_conflict_gate_passes(store, TARGET_EID).await.expect("冲突门只读判定")
}

async fn insert_equipment(pool: &PgPool) -> Uuid {
    let equipment_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, collected_at)
         VALUES ($1, 'main', '777001', 'Fixture Sword', 'weapon', now())",
    )
    .bind(equipment_id)
    .execute(pool)
    .await
    .expect("写入夹具装备");
    equipment_id
}

/// 写入一条未满足的结算 Send 投递并返回投递 ID；`reference` 与 `origin_id` 取同一个新义务 ID。
async fn insert_settlement_delivery(
    pool: &PgPool,
    template: &str,
    mut variables: Value,
    credits: i64,
    cod: i64,
    secret_refs: Value,
) -> Uuid {
    let obligation_id = Uuid::new_v4();
    variables["reference"] = json!(obligation_id.to_string());
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "template": template,
        "variables": variables,
        "credits": credits,
        "cod": cod,
        "equipment_secret_refs": secret_refs,
    });
    let delivery_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, decode(repeat('44', 32), 'hex'), 'settlement', $3, 'FixtureBuyer',
                 $4::jsonb, 'pending')",
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(obligation_id.to_string())
    .bind(payload.to_string())
    .execute(pool)
    .await
    .expect("写入未满足的结算投递");
    delivery_id
}

async fn delete_delivery(pool: &PgPool, delivery_id: Uuid) {
    sqlx::query("DELETE FROM public.deliveries WHERE id = $1")
        .bind(delivery_id)
        .execute(pool)
        .await
        .expect("删除夹具投递");
}

/// 生成方当前的全部结算 Send 形状都能被证明与目标装备无关；生成方从不产生的组合仍阻断。
#[tokio::test]
async fn postgres_current_settlement_payload_shapes_do_not_block_the_asset_conflict_gate() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let pool = admin.postgres_pool();
    let equipment_id = insert_equipment(pool).await;
    let equipment_refs = json!([{"kind": "auction_equipment_key", "id": equipment_id.to_string()}]);

    // 线上 2026-11-15 起生成的竞得装备 COD：v2 价格明细九变量加付款期限。
    insert_settlement_delivery(
        pool,
        "settlement_winner_equipment_cod",
        json!({
            "amount": "2205000",
            "item_name": "Fixture Sword",
            "adjusted_price": "2250000",
            "adjustment_ppm": "-20000",
            "hold_ppm": "940135",
            "pool_credit": "45000",
            "pricing_mode": "hold_curve_v2",
            "winning_bid": "2250000",
            "payment_deadline": DEADLINE,
        }),
        0,
        2_205_000,
        equipment_refs.clone(),
    )
    .await;
    assert!(gate_passes(&store).await, "带付款期限的 COD 不携带目标装备，不得阻断");

    insert_settlement_delivery(
        pool,
        "settlement_payment_carrier",
        json!({"amount": "1001", "item_name": "Fixture Sword", "payment_deadline": DEADLINE}),
        1,
        1_001,
        json!([]),
    )
    .await;
    for template in ["settlement_seller_payout", "settlement_buyer_refund"] {
        insert_settlement_delivery(
            pool,
            template,
            json!({"amount": "1000", "item_name": "Fixture Sword"}),
            1_000,
            0,
            json!([]),
        )
        .await;
    }
    // 写明装备名之前的旧两变量货款仍按原形状放行。
    insert_settlement_delivery(
        pool,
        "settlement_seller_payout",
        json!({"amount": "1000"}),
        1_000,
        0,
        json!([]),
    )
    .await;
    assert!(gate_passes(&store).await, "载体付款、货款与退款的展示变量不得阻断");

    // 生成方不会产生的组合无法证明与目标装备无关，逐个写入时都必须阻断，删除后恢复放行。
    let rejected = [
        (
            "settlement_seller_payout",
            json!({"amount": "1000", "item_name": "Fixture Sword", "payment_deadline": DEADLINE}),
            1_000,
            0,
            json!([]),
        ),
        (
            "settlement_payment_carrier",
            json!({"amount": "1001", "payment_deadline": DEADLINE}),
            1,
            1_001,
            json!([]),
        ),
        (
            "settlement_winner_equipment_cod",
            json!({"amount": "1000", "item_name": "Fixture Sword", "payment_deadline": DEADLINE}),
            0,
            1_000,
            equipment_refs.clone(),
        ),
    ];
    for (template, variables, credits, cod, refs) in rejected {
        let delivery_id =
            insert_settlement_delivery(pool, template, variables.clone(), credits, cod, refs).await;
        assert!(!gate_passes(&store).await, "{template} {variables} 必须阻断");
        delete_delivery(pool, delivery_id).await;
        assert!(gate_passes(&store).await, "删除 {template} 夹具后恢复放行");
    }

    store.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}
