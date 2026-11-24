//! 000134 新表的单列与复合外键最小违反行；世界外键由同表 CHECK 先拦截。

use super::support::Case;

fn fk(table: &'static str, constraint: &'static str, sql: &'static str) -> Case {
    Case { table, constraint, state: "23503", sql }
}

pub fn cases() -> Vec<Case> {
    vec![
        fk(
            "consignment_custody_assets",
            "c4_asset_auction_equipment_fk",
            "UPDATE public.consignment_custody_assets SET auction_equipment_id='deadbeef-0000-4000-8000-000000000001' WHERE equipment_id='85000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_custody_assets",
            "c4_asset_observation_fk",
            "UPDATE public.consignment_custody_assets SET first_observation_id='deadbeef-0000-4000-8000-000000000002' WHERE equipment_id='85000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "mail_disposition_attempts",
            "c4_attempt_credential_fk",
            "UPDATE public.mail_disposition_attempts SET credential_id='deadbeef-0000-4000-8000-000000000003' WHERE id='9a000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "mail_disposition_attempts",
            "c4_attempt_session_fk",
            "UPDATE public.mail_disposition_attempts SET session_id='deadbeef-0000-4000-8000-000000000005' WHERE id='9a000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_equipment_claims",
            "c4_claim_reservation_fk",
            "UPDATE public.consignment_equipment_claims SET reservation_id='deadbeef-0000-4000-8000-000000000007', position_id=NULL WHERE world_id='main' AND canonical_eid='42';",
        ),
        fk(
            "mail_observation_conflicts",
            "c4_conflict_observation_fk",
            "UPDATE public.mail_observation_conflicts SET observation_id='deadbeef-0000-4000-8000-000000000008' WHERE observation_id='84000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_custody_controls",
            "c4_control_command_fk",
            "UPDATE public.consignment_custody_controls SET command_id='deadbeef-0000-4000-8000-000000000009' WHERE id='a0000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_custody_controls",
            "c4_control_position_fk",
            "UPDATE public.consignment_custody_controls SET position_id='deadbeef-0000-4000-8000-00000000000a' WHERE id='a0000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_custody_controls",
            "c4_control_predecessor_fk",
            "UPDATE public.consignment_custody_controls SET predecessor_id='deadbeef-0000-4000-8000-00000000000b' WHERE id='a2000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_custody_events",
            "c4_custody_event_command_fk",
            "UPDATE public.consignment_custody_events SET command_id='deadbeef-0000-4000-8000-00000000000c' WHERE position_id='89000000-0000-0000-0000-000000000001' AND version=2;",
        ),
        fk(
            "consignment_custody_events",
            "c4_custody_event_listing_fk",
            "UPDATE public.consignment_custody_events SET listing_id='deadbeef-0000-4000-8000-00000000000d' WHERE position_id='89000000-0000-0000-0000-000000000001' AND version=2;",
        ),
        fk(
            "consignment_custody_events",
            "c4_custody_event_position_fk",
            "UPDATE public.consignment_custody_events SET position_id='deadbeef-0000-4000-8000-00000000000e' WHERE position_id='89000000-0000-0000-0000-000000000001' AND version=2;",
        ),
        fk(
            "mail_final_dispositions",
            "c4_final_command_fk",
            "UPDATE public.mail_final_dispositions SET command_id='deadbeef-0000-4000-8000-00000000000f' WHERE id='87000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_disposition_generations",
            "c4_generation_command_fk",
            "UPDATE public.consignment_disposition_generations SET created_command_id='deadbeef-0000-4000-8000-000000000010' WHERE position_id='89000000-0000-0000-0000-000000000001' AND generation=1;",
        ),
        fk(
            "consignment_disposition_generations",
            "c4_generation_position_fk",
            "UPDATE public.consignment_disposition_generations SET position_id='deadbeef-0000-4000-8000-000000000011' WHERE position_id='89000000-0000-0000-0000-000000000001' AND generation=1;",
        ),
        fk(
            "settlement_recovery_return_handoffs",
            "c4_handoff_mail_fk",
            "UPDATE public.settlement_recovery_return_handoffs SET return_mail_id='deadbeef-0000-4000-8000-000000000012' WHERE id='b9000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "settlement_recovery_return_handoffs",
            "c4_handoff_obligation_fk",
            "UPDATE public.settlement_recovery_return_handoffs SET obligation_id='deadbeef-0000-4000-8000-000000000013' WHERE id='b9000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "settlement_recovery_return_handoffs",
            "c4_handoff_recovery_fk",
            "UPDATE public.settlement_recovery_return_handoffs SET recovery_id='deadbeef-0000-4000-8000-000000000014' WHERE id='b9000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "mail_initial_decisions",
            "c4_initial_command_fk",
            "UPDATE public.mail_initial_decisions SET command_id='deadbeef-0000-4000-8000-000000000015' WHERE id='86000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_deposit_items",
            "c4_item_credential_fk",
            "UPDATE public.consignment_deposit_items SET credential_ref='deadbeef-0000-4000-8000-000000000017' WHERE id='88000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "mail_original_links",
            "c4_link_inbound_fk",
            "UPDATE public.mail_original_links SET inbound_id='deadbeef-0000-4000-8000-000000000019' WHERE inbound_id='8e000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_season_notices",
            "c4_notice_command_fk",
            "UPDATE public.consignment_season_notices SET command_id='deadbeef-0000-4000-8000-00000000001a' WHERE id='a4000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_season_notices",
            "c4_notice_season_fk",
            "UPDATE public.consignment_season_notices SET season_window_id='deadbeef-0000-4000-8000-00000000001b' WHERE id='a4000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "mail_disposition_proofs",
            "c4_proof_decision_fk",
            "UPDATE public.mail_disposition_proofs SET decision_id='deadbeef-0000-4000-8000-00000000001c' WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "mail_disposition_proofs",
            "c4_proof_observation_fk",
            "UPDATE public.mail_disposition_proofs SET observation_id='deadbeef-0000-4000-8000-00000000001d' WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "mail_disposition_proofs",
            "c4_proof_work_fk",
            "UPDATE public.mail_disposition_proofs SET work_id='deadbeef-0000-4000-8000-00000000001e' WHERE id='9b000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "mail_quarantine_rechecks",
            "c4_recheck_command_fk",
            "UPDATE public.mail_quarantine_rechecks SET command_id='deadbeef-0000-4000-8000-00000000001f' WHERE id='93000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "mail_quarantine_rechecks",
            "c4_recheck_quarantine_fk",
            "UPDATE public.mail_quarantine_rechecks SET quarantine_id='deadbeef-0000-4000-8000-000000000020' WHERE id='93000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "mail_quarantine_releases",
            "c4_release_command_fk",
            "UPDATE public.mail_quarantine_releases SET command_id='deadbeef-0000-4000-8000-000000000021' WHERE id='9f000000-0000-4000-8000-000000000001';",
        ),
        fk(
            "consignment_deposit_reservations",
            "c4_res_last_command_fk",
            "UPDATE public.consignment_deposit_reservations SET last_command_id='deadbeef-0000-4000-8000-000000000022' WHERE id='82000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "consignment_reservation_secrets",
            "c4_res_secret_reservation_fk",
            "UPDATE public.consignment_reservation_secrets SET reservation_id='deadbeef-0000-4000-8000-000000000024' WHERE reservation_id='82000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "mail_evidence_sessions",
            "c4_session_credential_fk",
            "UPDATE public.mail_evidence_sessions SET credential_id='deadbeef-0000-4000-8000-000000000025' WHERE id='81000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "mail_disposition_write_starts",
            "c4_start_command_fk",
            "UPDATE public.mail_disposition_write_starts SET command_id='deadbeef-0000-4000-8000-000000000026' WHERE work_id='8c000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "mail_disposition_write_starts",
            "c4_start_decision_fk",
            "UPDATE public.mail_disposition_write_starts SET decision_id='deadbeef-0000-4000-8000-000000000027' WHERE work_id='8c000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "mail_disposition_work",
            "c4_work_delivery_fk",
            "UPDATE public.mail_disposition_work SET delivery_id='deadbeef-0000-4000-8000-00000000002a' WHERE id='8c000000-0000-0000-0000-000000000001';",
        ),
        fk(
            "mail_disposition_work",
            "c4_work_notice_fk",
            "UPDATE public.mail_disposition_work SET action='send', bot_uid=NULL, world_id=NULL, external_mid=NULL, decision_id=NULL, notice_id='deadbeef-0000-4000-8000-00000000002b', expected_observation_id=NULL, manifest_digest=NULL WHERE id='8c000000-0000-0000-0000-000000000001';",
        ),
    ]
}

pub const EXCEPTIONS: &[(&str, &str)] = &[
    (
        "c4_start_delivery_fk",
        "不可达：改 start.delivery_id 同时破坏 c4_start_attempt_fk（含 delivery_id），而 attempt 的 delivery 又由 c4_attempt_delivery_fk 绑定到存在的 deliveries；无 proof 子行时仍是 attempt 外键先拦截",
    ),
    (
        "c4_start_work_fk",
        "不可达：改 start.work_id 同时破坏 c4_start_attempt_fk（含 work_id）；有 proof 子行时 c4_proof_start_fk 也会先拦截",
    ),
    (
        "c4_res_seller_fk",
        "恢复后的 c4_res_listing_fk 已包含 seller_user_id；不存在的卖家必先违反 listing 外键（名称在前）",
    ),
    (
        "c4_item_seller_fk",
        "恢复后的 c4_item_listing_fk 已包含 seller_user_id；不存在的卖家必先违反 listing 外键（名称在前）",
    ),
    (
        "c4_attempt_delivery_fk",
        "attempt 是 write_starts 复合外键的父行；改 delivery_id 先触发 c4_start_attempt_fk",
    ),
    (
        "c4_attempt_work_fk",
        "attempt 是 write_starts 复合外键的父行；改 work_id 先触发 c4_start_attempt_fk",
    ),
    ("c4_asset_world_fk", "world_id CHECK 只允许 main/isekai，FK 无法单独触发"),
    ("c4_claim_world_fk", "c4_claim_world_ck 先限制 world_id 闭集"),
    (
        "c4_final_world_fk",
        "不可达：非法 world 的终局必同时违反同 K 观察外键 c4_final_observation_fk（观察的 world CHECK 只允许 main/isekai），后者名称在前",
    ),
    ("c4_handoff_world_fk", "world_id 与资产复合 FK/CHECK 绑定，非法世界无法单独触发"),
    ("c4_initial_world_fk", "同表 world_id 参与首决定/观察绑定，非法世界先被 CHECK 拦截"),
    ("c4_item_world_fk", "item 的 world_id 由观察/预约/资产复合 FK 绑定，非法值先被复合 FK 拦截"),
    ("c4_link_world_fk", "link 的 world_id 由文本 CHECK 闭集限制"),
    ("c4_observation_world_fk", "observation 的 world_id 由文本 CHECK 闭集限制"),
    ("c4_original_world_fk", "original 的 world_id 由文本 CHECK 闭集限制"),
    ("c4_quarantine_world_fk", "quarantine 的 world_id 由文本 CHECK 闭集限制"),
    ("c4_readiness_world_fk", "readiness 的 world_id 由文本 CHECK 闭集限制"),
    ("c4_release_world_fk", "release 的 world_id 由文本 CHECK 闭集限制"),
    ("c4_res_world_fk", "reservation 的 world_id 由文本 CHECK 闭集限制"),
    ("c4_season_world_fk", "season 的 world_id CHECK 固定为 isekai"),
    ("c4_session_world_fk", "session 的 world_id 由文本 CHECK 闭集限制"),
];

fn rejected_command(command_id: &str) -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, result_status, result_code, result_body, created_at,
             target_kind, target_season_id, target_calendar_version)
         VALUES ('{command_id}', 2, 'worker', 'test-worker', 'season_notice',
                 '2026-isekai-1', decode(repeat('5c', 32), 'hex'), 409,
                 'stale_version',
                 '{{\"schema_version\":2,\"operation\":\"season_notice\",
                    \"disposition\":\"rejected\",\"command_id\":\"{command_id}\",
                    \"code\":\"stale_version\"}}',
                 transaction_timestamp(), 'season', '2026-isekai-1',
                 decode(repeat('5d', 32), 'hex'));"
    )
}

fn custody_event(command_id: &str, version: i64, proof_id: &str) -> String {
    format!(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('89000000-0000-0000-0000-000000000001', {version}, '{command_id}',
                 'reconcile', 'in_custody', 'reconcile', '{proof_id}',
                 '20000000-0000-0000-0000-000000000001', 4, transaction_timestamp());"
    )
}

#[tokio::test]
async fn postgres_command_foreign_keys_are_checked_at_commit() {
    use sqlx::PgPool;

    use super::support::seed_graph;
    use crate::support::TestDatabase;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool: PgPool = owner.postgres_pool().clone();
    seed_graph(&pool).await;

    let command = "deadbeef-0000-5000-8000-000000000031";
    let proof = "9b000000-0000-4000-8000-000000000001";
    super::super::support::execute(
        &pool,
        &format!("{} {}", custody_event(command, 3, proof), rejected_command(command)),
    )
    .await
    .expect("子事实先写、命令行后写应提交成功");

    let missing = "deadbeef-0000-5000-8000-000000000033";
    let missing_proof = "9b000000-0000-4000-8000-000000000001";
    let error = super::super::support::execute(&pool, &custody_event(missing, 4, missing_proof))
        .await
        .expect_err("缺少命令行必须在提交时失败");
    let database_error = error.as_database_error().expect("期望服务端 SQLSTATE");
    assert_eq!(database_error.code().as_deref(), Some("23503"));
    assert_eq!(database_error.constraint(), Some("c4_custody_event_command_fk"));
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_custody_events WHERE command_id=$1",
    )
    .bind(uuid::Uuid::parse_str(missing).expect("命令 UUID"))
    .fetch_one(&pool)
    .await
    .expect("读取回滚后的通知");
    assert_eq!(remaining, 0, "失败提交不能留下子事实");
}
