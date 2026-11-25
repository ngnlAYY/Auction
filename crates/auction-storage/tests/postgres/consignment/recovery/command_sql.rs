//! 000137 共享替换段测试的公共夹具：命令行与挂单版本 SQL，以及重列、直接撤回、灭失三条真实写序。
//!
//! 起点都是 `seed_main_custody` 加 `available_sql`：main 世界代管中的 position（处分代 1），挂单 available 版本 5。

use serde_json::{Value, json};

use super::super::contracts::fixture::{CONTRACT, Graph};
use super::super::deposit::support::{ACCEPTED, ASSET, LISTING, POSITION, PROOF, USER_1};
use super::{CASE, FACT, PROOF as LOSS_PROOF, case_sql, fact_sql, proof_sql};

pub(super) const PRICE: i64 = 101;
pub(super) const V6_AT: &str = "2026-01-01T06:00:00Z";
pub(super) const V7_AT: &str = "2026-01-01T07:00:00Z";
pub(super) const HOLD_COMMAND: &str = "c7e00000-0000-4000-8000-000000000001";
pub(super) const RELIST_COMMAND: &str = "c7e00000-0000-4000-8000-000000000002";
pub(super) const WITHDRAW_COMMAND: &str = "c7e00000-0000-4000-8000-000000000003";
pub(super) const CONFIRM_COMMAND: &str = "c7e00000-0000-4000-8000-000000000004";
pub(super) const LOSS_PROOF_COMMAND: &str = "c7e00000-0000-4000-8000-000000000005";
pub(super) const DELIVERED_COMMAND: &str = "c7e00000-0000-5000-8000-000000000006";
pub(super) const RECEIPT: &str = "7c700000-0000-4000-8000-000000000001";

/// 一条账本命令；`target_id` 为空表示浏览器目标，`listing` 为空表示不带挂单。
#[allow(clippy::too_many_arguments, reason = "账本行的列是固定的，拆成结构体只会增加样板")]
pub(super) fn command(
    id: &str,
    actor: (&str, &str),
    operation: &str,
    target: (&str, Option<&str>, &str),
    listing: Option<&str>,
    status: i32,
    body: &Value,
    at: &str,
) -> String {
    let quoted =
        |value: Option<&str>| value.map_or_else(|| "NULL".to_owned(), |v| format!("'{v}'"));
    let code = if status < 400 { "ok" } else { "rejected" };
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{id}', 2, '{kind}', '{actor_ref}', '{operation}', '{request_target}',
                 decode(repeat('d7', 32), 'hex'), '{target_kind}', {target_id}, {listing},
                 {status}, '{code}', '{body}', '{at}');",
        kind = actor.0,
        actor_ref = actor.1,
        target_kind = target.0,
        target_id = quoted(target.1),
        request_target = target.2,
        listing = quoted(listing),
    )
}

/// 挂单版本事件。
pub(super) fn event(
    version: i32,
    command: &str,
    (actor, kind): (&str, &str),
    (from, to): (&str, &str),
    reason: Option<&str>,
    at: &str,
) -> String {
    let reason = reason.map_or_else(|| "NULL".to_owned(), |value| format!("'{value}'"));
    format!(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', {version}, '{command}', '{actor}', '{kind}', '{from}', '{to}',
                 {reason}, {PRICE}, '{at}');"
    )
}

pub(super) fn listing_update(state: &str, reason: Option<&str>, version: i32, at: &str) -> String {
    let reason = reason.map_or_else(|| "NULL".to_owned(), |value| format!("'{value}'"));
    format!(
        "UPDATE public.consignment_listings
         SET state = '{state}', reason_code = {reason}, state_version = {version},
             updated_at = '{at}'
         WHERE id = '{LISTING}';"
    )
}

/// 处分代行；proof 复用取得证明（上代封闭证明在测试里与取得证明同一条）。
pub(super) fn generation(
    generation: i64,
    purpose: &str,
    predecessor: i64,
    command: &str,
) -> String {
    format!(
        "INSERT INTO public.consignment_disposition_generations
            (position_id, generation, purpose, predecessor, proof_id, service_epoch,
             blacklist_baseline_ids, season_window_id, created_command_id, created_at)
         VALUES ('{POSITION}', {generation}, '{purpose}', {predecessor}, '{PROOF}', 1,
                 ARRAY[]::uuid[], NULL, '{command}', '{V6_AT}');"
    )
}

/// 当前指针 CAS：position 状态不变，current_generation 前进到 g+1，版本 +1（D9 §5.3）。
pub(super) fn pointer(to_generation: i64, version: i64) -> String {
    format!(
        "UPDATE public.consignment_custody_positions
         SET current_generation = {to_generation}, version = {version}, updated_at = '{V6_AT}'
         WHERE id = '{POSITION}';"
    )
}

/// 运营止售：deposit_accepted -> held_off_market（版本 5）。
pub(super) fn hold_sql() -> String {
    let body = json!({"schema_version": 2, "operation": "admin_delist",
        "disposition": "hold_set", "command_id": HOLD_COMMAND, "listing_id": LISTING,
        "listing_version": 5, "state": "held_off_market", "held_reason": "operations_hold",
        "hold_id": "c1000000-0000-4000-8000-0000000000e1"});
    command(
        HOLD_COMMAND,
        ("admin", "fake-admin"),
        "admin_delist",
        ("listing", Some(LISTING), LISTING),
        Some(LISTING),
        200,
        &body,
        ACCEPTED,
    ) + &event(
        5,
        HOLD_COMMAND,
        ("admin", "hold_set"),
        ("deposit_accepted", "held_off_market"),
        Some("operations_hold"),
        ACCEPTED,
    ) + &listing_update("held_off_market", Some("operations_hold"), 5, ACCEPTED)
}

pub(super) const HOLD_AGAIN_COMMAND: &str = "c7e00000-0000-4000-8000-000000000011";

/// 重列后再次运营止售：available（版本 6）-> held_off_market（版本 7）。
pub(super) fn hold_again_sql() -> String {
    let body = json!({"schema_version": 2, "operation": "admin_delist",
        "disposition": "hold_set", "command_id": HOLD_AGAIN_COMMAND, "listing_id": LISTING,
        "listing_version": 7, "state": "held_off_market", "held_reason": "operations_hold",
        "hold_id": "c1000000-0000-4000-8000-0000000000e2"});
    command(
        HOLD_AGAIN_COMMAND,
        ("admin", "fake-admin"),
        "admin_delist",
        ("listing", Some(LISTING), LISTING),
        Some(LISTING),
        200,
        &body,
        V7_AT,
    ) + &event(
        7,
        HOLD_AGAIN_COMMAND,
        ("admin", "hold_set"),
        ("available", "held_off_market"),
        Some("operations_hold"),
        V7_AT,
    ) + &listing_update("held_off_market", Some("operations_hold"), 7, V7_AT)
}

pub(super) fn relist_command() -> String {
    let body = json!({"schema_version": 2, "operation": "relist", "disposition": "relisted",
        "command_id": RELIST_COMMAND, "listing_id": LISTING, "listing_version": 6,
        "state": "available", "price": PRICE, "generation": 2});
    command(
        RELIST_COMMAND,
        ("public_user", USER_1),
        "relist",
        ("listing", Some(LISTING), LISTING),
        Some(LISTING),
        200,
        &body,
        V6_AT,
    )
}

/// 重列版本：held_off_market -> available（版本 6）加 relisted 事件。
pub(super) fn relist_listing() -> String {
    event(
        6,
        RELIST_COMMAND,
        ("public_user", "relisted"),
        ("held_off_market", "available"),
        None,
        V6_AT,
    ) + &listing_update("available", None, 6, V6_AT)
}

/// 完整重列：命令、g+1 重列代、指针推进、事件与挂单版本同一事务。
pub(super) fn relist_sql() -> String {
    relist_command()
        + &generation(2, "relist", 1, RELIST_COMMAND)
        + &pointer(2, 3)
        + &relist_listing()
}

pub(super) fn withdraw_command() -> String {
    let body = json!({"schema_version": 2, "operation": "withdraw",
        "disposition": "withdrawal_accepted", "command_id": WITHDRAW_COMMAND,
        "listing_id": LISTING, "listing_version": 6, "state": "withdrawal_pending",
        "contract_id": CONTRACT, "equipment_id": ASSET});
    command(
        WITHDRAW_COMMAND,
        ("public_user", USER_1),
        "withdraw",
        ("listing", Some(LISTING), LISTING),
        Some(LISTING),
        202,
        &body,
        V6_AT,
    )
}

/// 撤回代上的 Withdrawal 合同图：挂在处分代 2，带前驱代 1 与封闭证明引用。
pub(super) fn withdrawal_graph() -> String {
    let sql = Graph::withdrawal().sql(WITHDRAW_COMMAND);
    let at_generation_two =
        sql.replacen(&format!("'{POSITION}', 1,"), &format!("'{POSITION}', 2,"), 1);
    let plain =
        format!("INSERT INTO public.consignment_withdrawals (contract_id) VALUES ('{CONTRACT}');");
    assert!(at_generation_two.contains(&plain) && at_generation_two != sql, "夹具形状变了");
    at_generation_two.replacen(
        &plain,
        &format!(
            "INSERT INTO public.consignment_withdrawals
                (contract_id, predecessor_generation, closure_proof_ref)
             VALUES ('{CONTRACT}', 1, 'fake-closure-proof');"
        ),
        1,
    )
}

pub(super) fn withdraw_listing() -> String {
    event(
        6,
        WITHDRAW_COMMAND,
        ("public_user", "withdrawal_accepted"),
        ("available", "withdrawal_pending"),
        None,
        V6_AT,
    ) + &listing_update("withdrawal_pending", None, 6, V6_AT)
}

/// 完整直接撤回：命令、g+1 撤回代、指针推进、Withdrawal 合同图、事件与挂单版本同一事务。
pub(super) fn withdraw_sql() -> String {
    withdraw_command()
        + &generation(2, "withdrawal_only", 1, WITHDRAW_COMMAND)
        + &pointer(2, 3)
        + &withdrawal_graph()
        + &withdraw_listing()
}

/// 直接撤回的事件部分：起始态与挂单新版本可指定（deposit_accepted 版本 4 → 5，available 版本 5 → 6）。
pub(super) fn withdraw_listing_at(from: &str, version: i32, actor: &str) -> String {
    event(
        version,
        WITHDRAW_COMMAND,
        (actor, "withdrawal_accepted"),
        (from, "withdrawal_pending"),
        None,
        V6_AT,
    ) + &listing_update("withdrawal_pending", None, version, V6_AT)
}

/// 从指定起始态直接撤回的完整写序；回执版本与挂单新版本一致。
pub(super) fn withdraw_sql_at(from: &str, version: i32, actor: &str) -> String {
    withdraw_command().replace("\"listing_version\":6", &format!("\"listing_version\":{version}"))
        + &generation(2, "withdrawal_only", 1, WITHDRAW_COMMAND)
        + &pointer(2, 3)
        + &withdrawal_graph()
        + &withdraw_listing_at(from, version, actor)
}

/// 灭失证据（Admin 录入的独立命令先行提交）。
pub(super) fn loss_proof_sql() -> String {
    let body = json!({"schema_version": 2, "operation": "record_position_loss_proof",
        "disposition": "loss_proof_recorded", "command_id": LOSS_PROOF_COMMAND,
        "position_id": POSITION, "proof_id": LOSS_PROOF, "proof_digest": "a3".repeat(32)});
    command(
        LOSS_PROOF_COMMAND,
        ("admin", "fake-admin"),
        "record_position_loss_proof",
        ("position", Some(POSITION), POSITION),
        None,
        200,
        &body,
        V6_AT,
    ) + &proof_sql(LOSS_PROOF, "fake-observation-1", LOSS_PROOF_COMMAND)
}

pub(super) fn confirm_command() -> String {
    let body = json!({"schema_version": 2, "operation": "confirm_position_loss",
        "disposition": "position_loss_recorded", "command_id": CONFIRM_COMMAND,
        "listing_id": LISTING, "listing_version": 6, "state": "closed",
        "position_id": POSITION, "position_version": 3, "loss_fact_id": FACT,
        "loss_case_id": CASE, "seller_receipt_id": RECEIPT});
    command(
        CONFIRM_COMMAND,
        ("admin", "fake-admin"),
        "confirm_position_loss",
        ("position", Some(POSITION), POSITION),
        None,
        200,
        &body,
        V6_AT,
    )
}

pub(super) fn position_lost() -> String {
    format!(
        "UPDATE public.consignment_custody_positions
         SET state = 'lost', version = 3, updated_at = '{V6_AT}'
         WHERE id = '{POSITION}';"
    )
}

pub(super) fn loss_facts() -> String {
    fact_sql(FACT, LOSS_PROOF, CONFIRM_COMMAND) + &case_sql(CASE, FACT, CONFIRM_COMMAND)
}

/// lost 代管事件；proof 是它引用的证明（正确值是损失证据）。
pub(super) fn lost_event(proof: &str) -> String {
    format!(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 3, '{CONFIRM_COMMAND}', 'lost', 'in_custody', 'lost', '{proof}',
                 '{LISTING}', 6, '{V6_AT}');"
    )
}

/// 占用释放；last_fact 是释放所依据的事实。
pub(super) fn claim_release(last_fact: &str) -> String {
    format!(
        "UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
             changed_at = '{V6_AT}', last_fact_id = '{last_fact}'
         WHERE world_id = 'main' AND canonical_eid = '42';"
    )
}

pub(super) fn loss_listing() -> String {
    event(
        6,
        CONFIRM_COMMAND,
        ("admin", "loss_confirmed"),
        ("available", "closed"),
        Some("loss_confirmed"),
        V6_AT,
    ) + &listing_update("closed", Some("loss_confirmed"), 6, V6_AT)
}

/// 完整灭失确认：命令、position lost、损失事实与案件、lost 事件、占用释放、挂单关闭同一事务。
pub(super) fn confirm_loss_sql() -> String {
    // 写序：损失事实先于 position 迁移（position 守卫在行级核对同事务事实）。
    confirm_command()
        + &loss_facts()
        + &position_lost()
        + &lost_event(LOSS_PROOF)
        + &claim_release(FACT)
        + &loss_listing()
}
