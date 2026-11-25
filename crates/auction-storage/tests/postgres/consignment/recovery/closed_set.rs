//! 000137 账本闭集：受限权利的浏览器 / 权利人 actor 与 target，以及新操作的回执形状。
//!
//! 行为边界：actor、目标与命令 ID 版本由账本 CHECK 失败关闭；回执形状由 `c4_valid_v2_receipt` 逐字段核对，
//! 缺字段、多余的资金字段与错误状态码都不得提交（不变量 6）。

use serde_json::{Value, json};
use sqlx::PgPool;

use super::super::deposit::support::{ACCEPTED, check, succeeds};
use super::command_sql::command;
use crate::support::TestDatabase;

const COMMAND: &str = "c7f00000-0000-4000-8000-000000000001";
const OTHER: &str = "c7f00000-0000-4000-8000-000000000002";
const CHALLENGE: &str = "c7f10000-0000-4000-8000-000000000001";
const RIGHT: &str = "c7f20000-0000-4000-8000-000000000001";
const EXPIRES: &str = "2026-01-01T03:10:00Z";

fn browser() -> String {
    "a1".repeat(32)
}

fn locate_body(id: &str) -> Value {
    json!({"schema_version": 2, "operation": "locate_right", "disposition": "request_received",
        "command_id": id, "challenge_id": CHALLENGE, "expires_at": EXPIRES})
}

fn locate(id: &str, actor_ref: &str, request_target: &str) -> String {
    command(
        id,
        ("browser", actor_ref),
        "locate_right",
        ("browser", None, request_target),
        None,
        202,
        &locate_body(id),
        ACCEPTED,
    )
}

fn claim(id: &str) -> String {
    let body = json!({"schema_version": 2, "operation": "claim_right",
        "disposition": "claim_accepted", "command_id": id, "right_id": RIGHT,
        "right_version": 1});
    command(
        id,
        ("right_holder", RIGHT),
        "claim_right",
        ("right", Some(RIGHT), RIGHT),
        None,
        202,
        &body,
        ACCEPTED,
    )
}

#[tokio::test]
async fn postgres_consignment_recovery_ledger_accepts_right_actors_and_fails_closed_on_shape() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();

    // 反例（各只违反一项）：actor_ref 不是 64 位小写十六进制；浏览器命令 ID 不是 v4；
    // 浏览器冒用权利人的操作；浏览器目标带了 UUID 目标；request_target 不是十六进制摘要；权利目标挂到别的目标种类。
    check(pool, &locate(COMMAND, "not-hex", &browser()), "consignment_command_actor").await;
    check(
        pool,
        &locate("c7f00000-0000-5000-8000-000000000001", &browser(), &browser()),
        "consignment_command_uuid_version",
    )
    .await;
    let borrowed =
        locate(COMMAND, &browser(), &browser()).replace("'locate_right'", "'claim_right'");
    check(pool, &borrowed, "consignment_command_actor").await;
    let with_target = locate(COMMAND, &browser(), &browser())
        .replace("'browser', NULL, NULL", &format!("'browser', '{RIGHT}', NULL"));
    check(pool, &with_target, "c4_command_typed_target_ck").await;
    check(pool, &locate(COMMAND, &browser(), "plain-text"), "consignment_command_target").await;
    let wrong_kind = claim(COMMAND).replace("'right', '", "'listing', '");
    check(pool, &wrong_kind, "consignment_command_target").await;
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM public.consignment_commands")
        .fetch_one(pool)
        .await
        .expect("读取命令数");
    assert_eq!(rows, 0, "全部反例回滚，账本无残留");

    // 正例：浏览器的找回与权利人的续办按各自的 actor、目标与回执形状入账。
    succeeds(pool, &(locate(COMMAND, &browser(), &browser()) + &claim(OTHER))).await;
}

async fn valid(pool: &PgPool, body: &Value, operation: &str, status: i32) -> bool {
    let command_id = body["command_id"].as_str().expect("夹具带命令 ID").to_owned();
    // 缺字段时 STRICT 校验函数返回 NULL：NULL 同样视为未通过。
    let result: Option<bool> =
        sqlx::query_scalar("SELECT public.c4_valid_v2_receipt($1, $2::uuid, $3, $4)")
            .bind(sqlx::types::Json(body))
            .bind(command_id)
            .bind(operation)
            .bind(status)
            .fetch_one(pool)
            .await
            .expect("调用回执校验函数");
    result == Some(true)
}

fn receipt(operation: &str, disposition: &str, extra: Value) -> Value {
    let mut body = json!({"schema_version": 2, "operation": operation,
        "disposition": disposition, "command_id": COMMAND});
    if let (Some(base), Some(more)) = (body.as_object_mut(), extra.as_object()) {
        base.extend(more.clone());
    }
    body
}

#[tokio::test]
async fn postgres_consignment_recovery_receipt_shapes_for_new_operations() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let id = |n: u8| format!("c7f30000-0000-4000-8000-0000000000{n:02}");
    let listing = json!({"listing_id": id(1), "listing_version": 6});

    // 每个操作的合法回执（正例）与最关键的反例：缺必填字段、带了不该有的资金字段、状态码不符。
    let relist = receipt(
        "relist",
        "relisted",
        json!({"listing_id": id(1), "listing_version": 6,
        "state": "available", "price": 101, "generation": 2}),
    );
    assert!(valid(pool, &relist, "relist", 200).await);
    let mut no_generation = relist.clone();
    no_generation.as_object_mut().expect("对象").remove("generation");
    assert!(!valid(pool, &no_generation, "relist", 200).await, "重列回执必须带新代序号");

    let mut accepted = receipt(
        "withdraw",
        "withdrawal_accepted",
        json!({"state": "withdrawal_pending",
        "contract_id": id(2), "equipment_id": id(3)}),
    );
    accepted.as_object_mut().expect("对象").extend(listing.as_object().expect("对象").clone());
    assert!(valid(pool, &accepted, "withdraw", 202).await);
    assert!(!valid(pool, &accepted, "withdraw", 200).await, "直接撤回回执只有 202");
    let mut with_intent = accepted.clone();
    with_intent["intent_id"] = json!(id(4));
    assert!(!valid(pool, &with_intent, "withdraw", 202).await, "直接撤回不带意愿");
    for missing in ["listing_id", "listing_version", "contract_id", "equipment_id"] {
        let mut incomplete = accepted.clone();
        incomplete.as_object_mut().expect("对象").remove(missing);
        assert!(!valid(pool, &incomplete, "withdraw", 202).await, "直接撤回回执缺 {missing}");
    }
    let mut wrong_state = accepted.clone();
    wrong_state["state"] = json!("available");
    assert!(
        !valid(pool, &wrong_state, "withdraw", 202).await,
        "直接撤回回执的状态只能是 withdrawal_pending"
    );
    let mut fulfilled = accepted.clone();
    fulfilled["operation"] = json!("fulfill_withdraw_intent");
    assert!(valid(pool, &fulfilled, "fulfill_withdraw_intent", 202).await);

    let confirm = receipt(
        "confirm_position_loss",
        "position_loss_recorded",
        json!({
        "listing_id": id(1), "listing_version": 6, "state": "closed", "position_id": id(5),
        "position_version": 3, "loss_fact_id": id(6), "loss_case_id": id(7),
        "seller_receipt_id": id(8)}),
    );
    assert!(valid(pool, &confirm, "confirm_position_loss", 200).await);
    let mut with_refund = confirm.clone();
    with_refund["refund_id"] = json!(id(9));
    assert!(
        !valid(pool, &with_refund, "confirm_position_loss", 200).await,
        "灭失回执不得带资金字段"
    );
    let mut with_generation = confirm.clone();
    with_generation["generation"] = json!(2);
    assert!(!valid(pool, &with_generation, "confirm_position_loss", 200).await, "灭失不新增处分代");

    let outcome = receipt(
        "record_loss_case_outcome",
        "case_outcome_recorded",
        json!({
        "case_id": id(7), "case_version": 2, "outcome_id": id(10), "seller_receipt_id": id(8)}),
    );
    assert!(valid(pool, &outcome, "record_loss_case_outcome", 200).await);
    let mut with_amount = outcome.clone();
    with_amount["amount"] = json!(5);
    assert!(!valid(pool, &with_amount, "record_loss_case_outcome", 200).await);

    let proof = receipt(
        "record_position_loss_proof",
        "loss_proof_recorded",
        json!({
        "position_id": id(5), "proof_id": id(11), "proof_digest": "a3".repeat(32)}),
    );
    assert!(valid(pool, &proof, "record_position_loss_proof", 200).await);
    let mut bad_digest = proof.clone();
    bad_digest["proof_digest"] = json!("A3");
    assert!(!valid(pool, &bad_digest, "record_position_loss_proof", 200).await);

    let issued = receipt(
        "verify_right",
        "right_verified",
        json!({"challenge_id": id(12),
        "right_id": id(13), "right_version": 1, "expires_at": EXPIRES}),
    );
    assert!(valid(pool, &issued, "verify_right", 200).await);
    let requested = receipt(
        "challenge_right",
        "request_received",
        json!({"challenge_id": id(12), "expires_at": EXPIRES}),
    );
    assert!(valid(pool, &requested, "challenge_right", 202).await);
    assert!(!valid(pool, &requested, "challenge_right", 200).await);
    let claimed = receipt(
        "claim_right",
        "claim_accepted",
        json!({"right_id": id(13),
        "right_version": 1}),
    );
    assert!(valid(pool, &claimed, "claim_right", 202).await);
    assert!(!valid(pool, &claimed, "claim_right", 200).await, "claim_accepted 只有 202");
    let completed = receipt(
        "claim_right",
        "already_completed",
        json!({"right_id": id(13),
        "right_version": 1}),
    );
    assert!(valid(pool, &completed, "claim_right", 200).await);

    // 新拒绝码各绑定自己的状态码。
    let rejected =
        |operation: &str, code: &str| receipt(operation, "rejected", json!({"code": code}));
    assert!(valid(pool, &rejected("verify_right", "challenge_invalid"), "verify_right", 403).await);
    assert!(valid(pool, &rejected("claim_right", "right_expired"), "claim_right", 409).await);
    assert!(
        !valid(pool, &rejected("verify_right", "challenge_invalid"), "verify_right", 409).await
    );
}
