//! 挑战与会话（失败次数、一次性消费、虚拟目标无会话、令牌不复用）、专用 outbox（密文形状、租约围栏、
//! 有结果即不可再领取）与提醒（每合同一次）的数据库约束（不变量 4、5、6、7）。

use super::super::deposit::support::{fails, succeeds};
use super::{
    CHALLENGE, RIGHT, T0, T10, WORK, challenge_notice_sql, challenge_sql, command_ids, digest,
    right_sql, seed, seed_notice,
};
use crate::support::TestDatabase;

fn attempt(number: i32, outcome: &str) -> String {
    format!(
        "INSERT INTO public.consignment_right_challenge_attempts
            (challenge_id, attempt, outcome, occurred_at)
         VALUES ('{CHALLENGE}', {number}, '{outcome}', '{T0}');"
    )
}

fn session(session_id: &str, challenge: &str, right: &str, expires_at: &str) -> String {
    format!(
        "INSERT INTO public.consignment_right_sessions
            (session_id, challenge_id, right_id, action_scope, browser_binding_digest, issued_at,
             expires_at)
         VALUES ('{session_id}', '{challenge}', '{right}', ARRAY['continue_order'], {d}, '{T0}',
                 '{expires_at}');",
        d = digest("b1")
    )
}

fn token(session_id: &str, generation: i32, byte: &str) -> String {
    format!(
        "INSERT INTO public.consignment_right_session_tokens
            (session_id, generation, token_digest, issued_at)
         VALUES ('{session_id}', {generation}, {d}, '{T0}');",
        d = digest(byte)
    )
}

#[tokio::test]
async fn postgres_consignment_right_challenge_allows_five_failures_and_one_consumption() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let command = command_ids(pool, 1).await.remove(0);
    succeeds(
        pool,
        &(right_sql(RIGHT, "c1", "continue_order", &command) + &challenge_sql(CHALLENGE, RIGHT)),
    )
    .await;

    // 正例：五次失败可记录；反例：第六次（即使是正确的消费）没有可写的尝试号。
    for number in 1..=5 {
        succeeds(pool, &attempt(number, "failed")).await;
    }
    fails(pool, &attempt(6, "consumed"), "23514", Some("c7_attempt_max_ck")).await;

    // 一个挑战至多消费一次：另一个挑战上两次 consumed 被部分唯一索引拒绝。
    let other = "7c500000-0000-4000-8000-000000000002";
    succeeds(pool, &challenge_sql(other, RIGHT)).await;
    let consume = |number: i32| {
        format!(
            "INSERT INTO public.consignment_right_challenge_attempts
                (challenge_id, attempt, outcome, occurred_at)
             VALUES ('{other}', {number}, 'consumed', '{T0}');"
        )
    };
    succeeds(pool, &consume(1)).await;
    fails(pool, &consume(2), "23505", Some("c7_attempt_consumed_uq")).await;
}

#[tokio::test]
async fn postgres_consignment_right_session_binds_real_challenge_and_never_extends() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let command = command_ids(pool, 1).await.remove(0);
    succeeds(
        pool,
        &(right_sql(RIGHT, "c1", "continue_order", &command) + &challenge_sql(CHALLENGE, RIGHT)),
    )
    .await;

    // 虚拟目标与真实目标同形，但永远换不到会话：虚拟挑战没有权利，会话外键不成立。
    let virtual_challenge = "7c500000-0000-4000-8000-000000000003";
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_right_challenges
                (challenge_id, right_id, is_virtual, right_version, browser_binding_digest,
                 created_at, expires_at)
             VALUES ('{virtual_challenge}', NULL, true, NULL, {d}, '{T0}', '{T10}');",
            d = digest("b1")
        ),
    )
    .await;
    fails(
        pool,
        &session("7c900000-0000-4000-8000-000000000009", virtual_challenge, RIGHT, T10),
        "23503",
        Some("c7_session_challenge_fk"),
    )
    .await;
    // 虚拟行不得携带真实目标才有的摘要。
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_right_challenges
                (challenge_id, right_id, is_virtual, right_version, browser_binding_digest,
                 code_mac, created_at, expires_at)
             VALUES ('7c500000-0000-4000-8000-000000000004', NULL, true, NULL, {d}, {d}, '{T0}',
                     '{T10}');",
            d = digest("b1")
        ),
        "23514",
        Some("c7_challenge_shape_ck"),
    )
    .await;

    // 正例：真实挑战换会话，截止不晚于签发后十分钟；反例：十一分钟被拒（轮换不延寿的上界）。
    let session_id = "7c900000-0000-4000-8000-000000000001";
    fails(
        pool,
        &session(session_id, CHALLENGE, RIGHT, "2026-01-03T00:11:00Z"),
        "23514",
        Some("c7_session_ttl_ck"),
    )
    .await;
    succeeds(pool, &(session(session_id, CHALLENGE, RIGHT, T10) + &token(session_id, 1, "d1")))
        .await;
    // 轮换：追加新代令牌；旧令牌摘要不得复用。
    succeeds(pool, &token(session_id, 2, "d2")).await;
    fails(pool, &token(session_id, 3, "d1"), "23505", Some("c7_session_token_uq")).await;
}

#[tokio::test]
async fn postgres_consignment_notice_outbox_keeps_code_sealed_and_never_reclaims_finished_work() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_notice(pool).await;

    // 反例：挑战码通知缺密文（等于把明文码另存别处的入口）被拒；无秘密的通知带密文也被拒。
    let plain = challenge_notice_sql("7c600000-0000-4000-8000-000000000002", CHALLENGE)
        .replace("decode('00ff', 'hex')", "NULL");
    fails(pool, &plain, "23514", Some("c7_notice_secret_ck")).await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_right_notice_outbox
                (work_id, notice_kind, right_id, frozen_identity, world_id, sealed_payload, nonce,
                 encryption_key_id, created_at)
             VALUES ('7c600000-0000-4000-8000-000000000003', 'right_notice', '{RIGHT}',
                     'FakeSellerTwo', 'main', decode('00ff', 'hex'), decode(repeat('0a', 12), 'hex'),
                     'fake-key-1', '{T0}');"
        ),
        "23514",
        Some("c7_notice_secret_ck"),
    )
    .await;

    let claim = |fence: i64| {
        format!(
            "UPDATE public.consignment_right_notice_outbox
             SET claim_token = '7c600000-0000-4000-8000-0000000000aa', claim_fence = {fence},
                 lease_expires_at = '{T10}'
             WHERE work_id = '{WORK}';"
        )
    };
    // 正例：领取使围栏加一；反例：围栏跳号被拒。
    succeeds(pool, &claim(1)).await;
    fails(pool, &claim(3), "23514", Some("c7_notice_fence")).await;
    // 反例：身份与密文列不可改。
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_right_notice_outbox SET frozen_identity = 'FakeOther'
             WHERE work_id = '{WORK}';"
        ),
        "23514",
        Some("c7_notice_immutable"),
    )
    .await;
    // 结果（含 unknown）一旦写下，工作不可再领取：未知结果永不自动重发。
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_right_notice_results (work_id, result, occurred_at)
             VALUES ('{WORK}', 'unknown', '{T0}');"
        ),
    )
    .await;
    fails(pool, &claim(2), "23514", Some("c7_notice_finished")).await;
}

#[tokio::test]
async fn postgres_consignment_payment_reminder_is_decided_once_per_contract() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let command = command_ids(pool, 1).await.remove(0);
    let reminder = |decision: &str| {
        format!(
            "INSERT INTO public.consignment_payment_reminders
                (contract_id, decision, due_at, decided_at, delivery_id, command_id)
             VALUES ('{contract}', '{decision}', '{T10}', '{T0}', NULL, '{command}');",
            contract = super::super::contracts::fixture::CONTRACT
        )
    };
    // 反例：排队决定必须带投递（没有投递就不是排队）。
    fails(pool, &reminder("queued"), "23514", Some("c7_reminder_delivery_ck")).await;
    // 正例：作废决定落库；反例：同一合同不能再有第二次决定（reminder_limit 固定 1）。
    succeeds(pool, &reminder("void_paid")).await;
    fails(pool, &reminder("void_cancelled"), "23505", Some("c7_reminder_pk")).await;
}
