//! 000137 段测试共用的夹具：main 代管 position 加合法 Sale 合同、损失链、权利与挑战行的 SQL 构造器。
//!
//! 自 `mod.rs` 析出，使 `postgres_audit` 目标能以符号链接复用而不带入本目录的测试模块。

use sqlx::PgPool;

use super::super::contracts::fixture::{CONTRACT, Graph, seed_main_custody};
use super::super::deposit::support::{ASSET, DECIDE, LISTING, POSITION, USER_1, USER_2, succeeds};

pub(in crate::consignment) const T0: &str = "2026-01-03T00:00:00Z";
pub(in crate::consignment) const T10: &str = "2026-01-03T00:10:00Z";
pub(in crate::consignment) const PROOF: &str = "7c200000-0000-4000-8000-000000000001";
pub(in crate::consignment) const FACT: &str = "7c300000-0000-4000-8000-000000000001";
pub(in crate::consignment) const CASE: &str = "7c400000-0000-4000-8000-000000000001";
pub(in crate::consignment) const RIGHT: &str = "7c100000-0000-4000-8000-000000000001";
pub(in crate::consignment) const CHALLENGE: &str = "7c500000-0000-4000-8000-000000000001";
pub(in crate::consignment) const WORK: &str = "7c600000-0000-4000-8000-000000000001";

/// main 代管 position 加一份合法 Sale 合同图。
pub(in crate::consignment) async fn seed(pool: &PgPool) {
    seed_main_custody(pool).await;
    succeeds(pool, &Graph::sale_main().sql(DECIDE)).await;
}

/// 账本里已有的命令 ID（按时间与 ID 排序），供只需要“存在一条命令”的外键使用。
pub(in crate::consignment) async fn command_ids(pool: &PgPool, count: usize) -> Vec<String> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT command_id::text FROM public.consignment_commands
         ORDER BY created_at, command_id LIMIT $1",
    )
    .bind(i64::try_from(count).expect("命令数"))
    .fetch_all(pool)
    .await
    .expect("读取命令账本");
    assert_eq!(ids.len(), count, "夹具应提供至少 {count} 条命令");
    ids
}

pub(in crate::consignment) fn digest(byte: &str) -> String {
    format!("decode(repeat('{byte}', 32), 'hex')")
}

/// 一份损失证据；source 区分同一 position 的不同证据来源。
pub(in crate::consignment) fn proof_sql(proof: &str, source: &str, command: &str) -> String {
    format!(
        "INSERT INTO public.consignment_loss_proofs
            (proof_id, position_id, world_id, canonical_eid, last_known_ref, movement_digest,
             exclusion_digest, source_kind, observation_ref, digest, origin_command_id, recorded_at)
         VALUES ('{proof}', '{POSITION}', 'main', '42', 'fake-last-known', {m}, {e}, 'fake-source',
                 '{source}', {d}, '{command}', '{T0}');",
        m = digest("a1"),
        e = digest("a2"),
        d = digest("a3"),
    )
}

pub(in crate::consignment) fn fact_sql(fact: &str, proof: &str, command: &str) -> String {
    format!(
        "INSERT INTO public.consignment_loss_facts
            (loss_fact_id, position_id, proof_id, listing_id, seller_user_id, equipment_id,
             world_id, canonical_eid, generation, origin_command_id, occurred_at)
         VALUES ('{fact}', '{POSITION}', '{proof}', '{LISTING}', '{USER_1}', '{ASSET}', 'main',
                 '42', 1, '{command}', '{T0}');"
    )
}

pub(in crate::consignment) fn case_sql(case: &str, fact: &str, command: &str) -> String {
    format!(
        "INSERT INTO public.consignment_loss_cases
            (case_id, loss_fact_id, position_id, seller_user_id, origin_command_id, created_at)
         VALUES ('{case}', '{fact}', '{POSITION}', '{USER_1}', '{command}', '{T0}');"
    )
}

/// 证据、事实、案件一条链（命令取 commands[0..3]）。
pub(in crate::consignment) fn loss_chain_sql(commands: &[String]) -> String {
    proof_sql(PROOF, "fake-observation-1", &commands[0])
        + &fact_sql(FACT, PROOF, &commands[1])
        + &case_sql(CASE, FACT, &commands[2])
}

/// 买方权：Sale 合同的买家，定位符摘要由 locator 字节决定。
pub(in crate::consignment) fn right_sql(
    right: &str,
    locator: &str,
    kind: &str,
    command: &str,
) -> String {
    format!(
        "INSERT INTO public.consignment_rights
            (right_id, locator_digest, beneficiary_user_id, frozen_identity, world_id, kind,
             contract_id, position_id, created_command_id, created_at)
         VALUES ('{right}', {d}, '{USER_2}', 'FakeSellerTwo', 'main', '{kind}', '{CONTRACT}', NULL,
                 '{command}', '{T0}');
         INSERT INTO public.consignment_right_states
            (right_id, version, state, allowed_actions, occurred_at)
         VALUES ('{right}', 1, 'active', ARRAY['continue_order'], '{T0}');",
        d = digest(locator),
    )
}

/// 真实挑战：绑定权利版本 1，十分钟有效。
pub(in crate::consignment) fn challenge_sql(challenge: &str, right: &str) -> String {
    format!(
        "INSERT INTO public.consignment_right_challenges
            (challenge_id, right_id, is_virtual, right_version, browser_binding_digest, code_mac,
             nonce_digest, identity_digest, created_at, expires_at)
         VALUES ('{challenge}', '{right}', false, 1, {b}, {m}, {n}, {i}, '{T0}', '{T10}');",
        b = digest("b1"),
        m = digest("b2"),
        n = digest("b3"),
        i = digest("b4"),
    )
}

/// 挑战码通知：只含密文三元组，不含明文。
pub(in crate::consignment) fn challenge_notice_sql(work: &str, challenge: &str) -> String {
    format!(
        "INSERT INTO public.consignment_right_notice_outbox
            (work_id, notice_kind, challenge_id, frozen_identity, world_id, sealed_payload, nonce,
             encryption_key_id, created_at)
         VALUES ('{work}', 'challenge', '{challenge}', 'FakeSellerTwo', 'main',
                 decode('00ff', 'hex'), decode(repeat('0a', 12), 'hex'), 'fake-key-1', '{T0}');"
    )
}

/// 权利、挑战与待发的挑战码通知。
pub(in crate::consignment) async fn seed_notice(pool: &PgPool) {
    seed(pool).await;
    let commands = command_ids(pool, 1).await;
    succeeds(
        pool,
        &(right_sql(RIGHT, "c1", "continue_order", &commands[0])
            + &challenge_sql(CHALLENGE, RIGHT)
            + &challenge_notice_sql(WORK, CHALLENGE)),
    )
    .await;
}
