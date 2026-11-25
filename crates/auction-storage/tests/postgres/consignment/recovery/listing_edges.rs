//! 挂单新边：held_off_market -> available（重列）与 withdrawal_pending -> closed(withdrawn_delivered)
//! 只在同事务命令、事件与事实齐备时通过；sold 的既有边仍要求同事务确领。灭失关闭见 `claim_release`。
//!
//! 保护的不变量：3（版本、事件、命令同提）、6（失败关闭）。

use serde_json::json;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1, seed_main_custody};
use super::super::deposit::support::{
    ASSET, HANDOFF_EDGE, LISTING, POSITION, USER_1, fails, succeeds,
};
use super::super::ledger136::fixture::{available_sql, seed_purchased};
use super::command_sql::{
    DELIVERED_COMMAND, HOLD_COMMAND, RELIST_COMMAND, V6_AT, V7_AT, command, event, generation,
    hold_again_sql, hold_sql, listing_update, pointer, relist_command, relist_listing, relist_sql,
    withdraw_command, withdraw_listing_at, withdraw_sql, withdraw_sql_at,
};
use crate::support::TestDatabase;

#[tokio::test]
async fn postgres_consignment_recovery_relist_edge_needs_same_transaction_command_and_event() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &hold_sql()).await;

    // 反例：只改挂单版本，没有重列命令与事件。
    fails(
        pool,
        &listing_update("available", None, 6, V6_AT),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;
    // 反例：事件绑定的命令不是重列命令（借用已提交的止售命令）。
    fails(
        pool,
        &(event(
            6,
            HOLD_COMMAND,
            ("public_user", "relisted"),
            ("held_off_market", "available"),
            None,
            V6_AT,
        ) + &listing_update("available", None, 6, V6_AT)),
        "23514",
        None,
    )
    .await;
    // 反例：重列命令的回执版本与挂单新版本不符。
    fails(
        pool,
        &(relist_command().replace("\"listing_version\":6", "\"listing_version\":9")
            + &generation(2, "relist", 1, RELIST_COMMAND)
            + &pointer(2, 3)
            + &relist_listing()),
        "23514",
        Some("consignment_command_version_event"),
    )
    .await;
    // 正例。
    succeeds(pool, &relist_sql()).await;
}

/// 已证原卖家领取：consignment_withdrawn 交接边（挂在本挂单的 position 上）。
fn withdrawn_edge() -> String {
    format!(
        "INSERT INTO public.consignment_equipment_handoffs
            (id, equipment_id, world_id, canonical_eid, season_id, predecessor_id,
             from_user_id, to_user_id, kind, auction_lot_id, position_id, proof_ref, occurred_at)
         VALUES ('{HANDOFF_EDGE}', '{ASSET}', 'main', '42', NULL, NULL, '{USER_1}', '{USER_1}',
                 'consignment_withdrawn', NULL, '{POSITION}', 'controlled-delivery-proof-ref',
                 '{V7_AT}');"
    )
}

fn delivered_listing() -> String {
    let body = json!({"schema_version": 2, "operation": "prove_handoff",
        "disposition": "original_claimed", "command_id": DELIVERED_COMMAND,
        "contract_id": CONTRACT, "contract_version": 2, "obligation_id": OBLIGATION_1,
        "proof_id": "c7e30000-0000-4000-8000-000000000001", "listing_id": LISTING,
        "listing_version": 7, "state": "closed"});
    command(
        DELIVERED_COMMAND,
        ("worker", "fake-worker"),
        "prove_handoff",
        ("obligation", Some(OBLIGATION_1), OBLIGATION_1),
        None,
        200,
        &body,
        V7_AT,
    ) + &event(
        7,
        DELIVERED_COMMAND,
        ("worker", "withdrawn_delivered"),
        ("withdrawal_pending", "closed"),
        Some("withdrawn_delivered"),
        V7_AT,
    ) + &listing_update("closed", Some("withdrawn_delivered"), 7, V7_AT)
}

#[tokio::test]
async fn postgres_consignment_recovery_withdrawn_delivered_needs_the_withdrawn_edge() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &available_sql()).await;
    succeeds(pool, &withdraw_sql()).await;

    // 反例：命令与事件都在，但没有同事务的 consignment_withdrawn 交接边。
    fails(pool, &delivered_listing(), "23514", Some("consignment_listing_transition")).await;
    // 反例：没有命令与事件，只有交接边与挂单版本。
    fails(
        pool,
        &(withdrawn_edge() + &listing_update("closed", Some("withdrawn_delivered"), 7, V7_AT)),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;
    // 正例：交接边、命令、事件与挂单版本同事务。
    succeeds(pool, &(withdrawn_edge() + &delivered_listing())).await;
    let state: String = sqlx::query_scalar("SELECT reason_code FROM public.consignment_listings")
        .fetch_one(pool)
        .await
        .expect("读取关闭原因");
    assert_eq!(state, "withdrawn_delivered");
}

#[tokio::test]
async fn postgres_consignment_recovery_sold_close_still_needs_handoff() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_purchased(pool).await;
    // 反例：已成交待履约的挂单没有确领（position 同事务 handed_over）就直接关闭为 sold。
    fails(
        pool,
        &listing_update("closed", Some("sold"), 7, V7_AT),
        "23514",
        Some("consignment_listing_transition"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_recovery_withdrawal_accepted_event_binds_the_withdraw_command() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &available_sql()).await;

    // 反例：事件与挂单版本在，命令回执的版本与挂单新版本不符。
    fails(
        pool,
        &withdraw_sql_at("available", 6, "public_user")
            .replace("\"listing_version\":6", "\"listing_version\":9"),
        "23514",
        None,
    )
    .await;
    // 反例：没有同事务的 Withdrawal 合同（只有命令、事件与挂单版本）。
    fails(
        pool,
        &(withdraw_command() + &withdraw_listing_at("available", 6, "public_user")),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;
    // 反例：事件的 actor 不是 public_user（也不是 worker 的 fulfill_withdraw_intent）。
    fails(pool, &withdraw_sql_at("available", 6, "admin"), "23514", Some("consignment_event_edge"))
        .await;
    // 反例：撤回带了原因码（直接撤回的事件与挂单版本不带 reason_code）。
    fails(
        pool,
        &withdraw_sql_at("available", 6, "public_user").replace(
            "NULL, 101, '2026-01-01T06:00:00Z');",
            "'operations_hold', 101, '2026-01-01T06:00:00Z');",
        ),
        "23514",
        Some("consignment_event_edge"),
    )
    .await;
    // 正例：available -> withdrawal_pending。
    succeeds(pool, &withdraw_sql_at("available", 6, "public_user")).await;
}

#[tokio::test]
async fn postgres_consignment_recovery_withdrawal_accepted_edge_from_deposit_accepted() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    // 挂单停在 deposit_accepted（版本 4），position 在库：已在库而尚未上架即直接撤回。
    seed_main_custody(pool).await;

    // 反例：事件 actor 不属于 deposit_accepted 的 withdrawal_accepted 边。
    fails(
        pool,
        &withdraw_sql_at("deposit_accepted", 5, "admin"),
        "23514",
        Some("consignment_event_edge"),
    )
    .await;
    // 正例：deposit_accepted -> withdrawal_pending。
    succeeds(pool, &withdraw_sql_at("deposit_accepted", 5, "public_user")).await;
    let state: String = sqlx::query_scalar("SELECT state FROM public.consignment_listings")
        .fetch_one(pool)
        .await
        .expect("读取挂单状态");
    assert_eq!(state, "withdrawal_pending");
}

#[tokio::test]
async fn postgres_consignment_recovery_relist_edge_rejects_a_relist_from_an_earlier_transaction() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &hold_sql()).await;
    succeeds(pool, &relist_sql()).await;
    succeeds(pool, &hold_again_sql()).await;

    // 反例：第二次 held_off_market -> available 复用前一事务已提交的重列命令、g+1 代与 relisted 事件的身份。
    fails(
        pool,
        &(event(
            8,
            RELIST_COMMAND,
            ("public_user", "relisted"),
            ("held_off_market", "available"),
            None,
            V7_AT,
        ) + &listing_update("available", None, 8, V7_AT)),
        "23514",
        None,
    )
    .await;
}
