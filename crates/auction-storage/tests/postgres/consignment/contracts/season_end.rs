//! position 季末终态：仅 in_custody / reconcile 在季末等号及之后可进入，占用释放只对该终态免邮件证明。

use sqlx::PgPool;

use super::super::deposit::support::{
    ACCEPTED, ATTEMPT, CREDENTIAL, DECIDE, DECISION, DELIVERY, LISTING, POSITION, PROOF,
    PROOF_COMMAND, SESSION, WORK, attempt_start_proof_and_custody, check, fails, seed_accepted,
    succeeds,
};
use super::fixture::{WINDOW, seed_isekai_awaiting, seed_isekai_custody};
use crate::support::TestDatabase;

const FUTURE: &str = "transaction_timestamp() + interval '10 days'";
const PAST: &str = "transaction_timestamp() - interval '1 day'";

/// 把季窗结束时刻改到给定表达式（事实表的只追加触发器在本事务内停用，仅测试用）。
fn window_end(expression: &str) -> String {
    format!(
        "ALTER TABLE public.consignment_season_windows DISABLE TRIGGER USER;
         UPDATE public.consignment_season_windows
         SET end_utc = {expression}, admission_cutoff_utc = transaction_timestamp() - interval '2 days'
         WHERE id = '{WINDOW}';"
    )
}

fn event(version: i32, kind: &str, from_state: &str, to_state: &str, proof: &str) -> String {
    format!(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', {version}, '{PROOF_COMMAND}', '{kind}', '{from_state}',
                 '{to_state}', {proof}, '{LISTING}', 4, transaction_timestamp());"
    )
}

/// position 终态更新加事件加占用释放；version 是新的 position 版本。
fn season_end(version: i32, from_state: &str) -> String {
    format!(
        "UPDATE public.consignment_custody_positions
         SET state = 'season_ended', version = {version}, updated_at = transaction_timestamp()
         WHERE id = '{POSITION}';
         {event}
         UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
             changed_at = transaction_timestamp(), last_fact_id = '{POSITION}'
         WHERE world_id = 'isekai' AND canonical_eid = '42';",
        event = event(version, "season_ended", from_state, "season_ended", "NULL")
    )
}

async fn state_and_holder(pool: &PgPool) -> (String, Option<String>) {
    sqlx::query_as(
        "SELECT position.state,
                (SELECT claim.position_id::text FROM public.consignment_equipment_claims AS claim
                 WHERE claim.canonical_eid = '42')
         FROM public.consignment_custody_positions AS position WHERE position.id = $1::uuid",
    )
    .bind(POSITION)
    .fetch_one(pool)
    .await
    .expect("读取 position 与占用")
}

#[tokio::test]
async fn postgres_consignment_season_end_requires_the_season_boundary() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_isekai_custody(pool, FUTURE).await;

    // 反例：季窗未结束、差一微秒到等号，都被位置守卫拒绝，占用也不释放。
    fails(pool, &season_end(3, "in_custody"), "23514", Some("c5_position_season_end")).await;
    let just_before = format!(
        "{}{}",
        window_end("transaction_timestamp() + interval '1 microsecond'"),
        season_end(3, "in_custody")
    );
    fails(pool, &just_before, "23514", Some("c5_position_season_end")).await;
    assert_eq!(state_and_holder(pool).await, ("in_custody".to_owned(), Some(POSITION.to_owned())));

    // lost 只在同事务写入损失事实时放行（000137）；没有事实仍被拒绝。
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_custody_positions
             SET state = 'lost', version = 3, updated_at = transaction_timestamp()
             WHERE id = '{POSITION}';"
        ),
        "23514",
        Some("c7_position_loss_fact"),
    )
    .await;

    // 事件层：季末事件不得带邮件证明；lost 事件只能从仍占用实物的四态进入（000137；000136 起 awaiting_take
    // 可季末终结，committed 等事件种类也已放开，由 ledger136 覆盖）。
    check(
        pool,
        &event(3, "season_ended", "in_custody", "season_ended", &format!("'{PROOF}'")),
        "c7_custody_event_season_ended_ck",
    )
    .await;
    check(
        pool,
        &event(3, "lost", "awaiting_take", "lost", &format!("'{PROOF}'")),
        "c4_custody_event_state_ck",
    )
    .await;

    // 正例：恰好等于季末（等号即失效）即可终结，并在同一事务释放占用。
    succeeds(
        pool,
        &format!("{}{}", window_end("transaction_timestamp()"), season_end(3, "in_custody")),
    )
    .await;
    assert_eq!(state_and_holder(pool).await, ("season_ended".to_owned(), None));
}

#[tokio::test]
async fn postgres_consignment_season_end_accepts_reconcile_after_the_boundary() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_isekai_custody(pool, FUTURE).await;
    succeeds(
        pool,
        &format!(
            "UPDATE public.consignment_custody_positions
             SET state = 'reconcile', version = 3, updated_at = '{ACCEPTED}'
             WHERE id = '{POSITION}';
             {}",
            event(3, "reconcile", "in_custody", "reconcile", &format!("'{PROOF}'"))
        ),
    )
    .await;
    // 季窗此刻结束：reconcile 也在季末终结（决定 21：季末清空不赔）。
    succeeds(pool, &format!("{}{}", window_end(PAST), season_end(4, "reconcile"))).await;
    assert_eq!(state_and_holder(pool).await, ("season_ended".to_owned(), None));
}

#[tokio::test]
async fn postgres_consignment_claim_release_without_proof_is_only_for_season_end() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_isekai_custody(pool, FUTURE).await;

    // 反例：季窗已结束，但 position 没有同事务终态，或占用事实指向别处，释放都被拒绝。
    let release = "UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
             changed_at = transaction_timestamp(), last_fact_id = '{fact}'
         WHERE world_id = 'isekai' AND canonical_eid = '42';";
    let no_terminal = format!(
        "{}{}{}",
        window_end(PAST),
        event(3, "season_ended", "in_custody", "season_ended", "NULL"),
        release.replace("{fact}", POSITION)
    );
    fails(pool, &no_terminal, "23514", Some("c5_claim_season_end_fact")).await;
    let wrong_fact = season_end(3, "in_custody")
        .replace(&format!("last_fact_id = '{POSITION}'"), &format!("last_fact_id = '{LISTING}'"));
    fails(
        pool,
        &format!("{}{wrong_fact}", window_end(PAST)),
        "23514",
        Some("c5_claim_season_end_fact"),
    )
    .await;
    assert_eq!(state_and_holder(pool).await, ("in_custody".to_owned(), Some(POSITION.to_owned())));
}

#[tokio::test]
async fn postgres_consignment_season_end_rejects_main_world_and_untaken_positions() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_accepted(pool).await;

    // 反例：main 世界没有季窗，awaiting_take 不能终结（季窗条件的反例见 awaiting_take 专项用例）。
    let from_awaiting = format!(
        "UPDATE public.consignment_custody_positions
         SET state = 'season_ended', version = 2, updated_at = transaction_timestamp()
         WHERE id = '{POSITION}';"
    );
    fails(pool, &from_awaiting, "23514", Some("c6_position_take_season_end")).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    // 反例：main 世界没有季窗，in_custody 也不能终结。
    fails(
        pool,
        &season_end(3, "in_custody").replace("'isekai'", "'main'"),
        "23514",
        Some("c5_position_season_end"),
    )
    .await;
}

/// 取得工作的关闭记录（R5-C3：同事务写入才允许终结）。
fn work_closure() -> String {
    format!(
        "INSERT INTO public.consignment_work_closures (work_id, position_id, reason)
         VALUES ('{WORK}', '{POSITION}', 'season_ended');"
    )
}

/// 未取得（awaiting_take）的 position 季末终结：position 更新、季末事件与占用释放。
fn awaiting_season_end() -> String {
    format!(
        "
         UPDATE public.consignment_custody_positions
         SET state = 'season_ended', version = 2, updated_at = transaction_timestamp()
         WHERE id = '{POSITION}';
         INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 2, '{DECIDE}', 'season_ended', 'awaiting_take', 'season_ended',
                 NULL, '{LISTING}', 4, transaction_timestamp());
         UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
             changed_at = transaction_timestamp(), last_fact_id = '{POSITION}'
         WHERE world_id = 'isekai' AND canonical_eid = '42';"
    )
}

/// 取得工作的起写事实（命令、尝试与起写）；与关闭记录同事务时，关闭记录必须先写。
fn write_start() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{PROOF_COMMAND}', 2, 'worker', 'fake-worker', 'apply_mail_proof',
                 '{ATTEMPT}', decode(repeat('a5', 32), 'hex'), 'original_attempt',
                 '{ATTEMPT}', NULL, 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"apply_mail_proof\",
                    \"disposition\":\"take_proven\",\"command_id\":\"{PROOF_COMMAND}\",
                    \"decision_id\":\"{DECISION}\",\"proof_id\":\"{PROOF}\"}}',
                 '{ACCEPTED}');
         INSERT INTO public.mail_disposition_attempts
            (id, work_id, delivery_id, fence, credential_id, credential_version,
             runtime_generation, session_id, expected_digest, prepared_at)
         VALUES ('{ATTEMPT}', '{WORK}', '{DELIVERY}', 1, '{CREDENTIAL}', 1, 1,
                 '{SESSION}', decode(repeat('a6', 32), 'hex'), '{ACCEPTED}');
         INSERT INTO public.mail_disposition_write_starts
            (work_id, decision_id, delivery_id, attempt_id, fence, started_at, command_id)
         VALUES ('{WORK}', '{DECISION}', '{DELIVERY}', '{ATTEMPT}', 1, '{ACCEPTED}',
                 '{PROOF_COMMAND}');"
    )
}

#[tokio::test]
async fn postgres_consignment_season_end_closes_untaken_work_only_when_unstarted_and_closed() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_isekai_awaiting(pool, PAST).await;

    // 反例（同事务关闭记录）：季窗已结束、未起写，但没有本事务的关闭记录，不得终结。
    fails(pool, &awaiting_season_end(), "23514", Some("c6_position_take_season_end")).await;
    // 反例（已起写不得终结）：先写关闭记录、再起写，同事务终结仍被位置守卫拒绝
    // （关闭记录自身的触发器只在写入时核对起写，起写在其后不会被它拦下）。
    fails(
        pool,
        &format!("{}{}{}", work_closure(), write_start(), awaiting_season_end()),
        "23514",
        Some("c6_position_take_season_end"),
    )
    .await;
    assert_eq!(
        state_and_holder(pool).await,
        ("awaiting_take".to_owned(), Some(POSITION.to_owned()))
    );

    // 正例：季窗已结束、未起写、同事务写入关闭记录，进入 season_ended 并释放占用。
    succeeds(pool, &format!("{}{}", work_closure(), awaiting_season_end())).await;
    assert_eq!(state_and_holder(pool).await, ("season_ended".to_owned(), None));
}
