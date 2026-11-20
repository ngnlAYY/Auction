//! 一口价挂单基础迁移的 PostgreSQL 约束、历史证据和运行账号权限。

use sqlx::{AssertSqlSafe, PgPool};

use crate::support::{TestDatabase, fixtures};

const USER_1: &str = "10000000-0000-0000-0000-000000000001";
const USER_2: &str = "10000000-0000-0000-0000-000000000002";
const LISTING_1: &str = "20000000-0000-0000-0000-000000000001";
const LISTING_2: &str = "20000000-0000-0000-0000-000000000002";
const LISTING_3: &str = "20000000-0000-0000-0000-000000000003";
const SNAPSHOT_1: &str = "30000000-0000-0000-0000-000000000001";
const SNAPSHOT_2: &str = "30000000-0000-0000-0000-000000000002";
const REGISTER_1: &str = "40000000-0000-0000-0000-000000000001";
const REGISTER_2: &str = "40000000-0000-0000-0000-000000000002";
const REGISTER_3: &str = "40000000-0000-0000-0000-000000000003";
const LOOKUP_1: &str = "41000000-0000-0000-0000-000000000001";
const LOOKUP_2: &str = "41000000-0000-0000-0000-000000000002";
const WITHDRAW: &str = "42000000-0000-0000-0000-000000000001";
const WORK: &str = "50000000-0000-0000-0000-000000000001";
const LOT_1: &str = "60000000-0000-0000-0000-000000000001";
const LOT_2: &str = "60000000-0000-0000-0000-000000000002";
const WRONG_EID_LOT: &str = "60000000-0000-0000-0000-000000000003";
const WRONG_WORLD_LOT: &str = "60000000-0000-0000-0000-000000000004";
const REPLAY_1: &str = "90000000-0000-0000-0000-000000000001";
const REPLAY_2: &str = "90000000-0000-0000-0000-000000000002";
const NIL: &str = "00000000-0000-0000-0000-000000000000";
const START: &str = "2026-01-01T00:00:00Z";
const LATER: &str = "2026-01-01T01:00:00Z";
const END: &str = "2026-01-02T00:00:00Z";
const BEFORE_END: &str = "2026-01-01T23:59:59.999999Z";
const AFTER_END: &str = "2026-01-02T00:00:00.000001Z";
const PUBLISHED_END: &str = "2026-01-08T00:00:00Z";

async fn execute(pool: &PgPool, sql: &str) -> Result<(), sqlx::Error> {
    // 多条语句放在同一显式事务，保证延迟约束确实在 commit 中检查。
    let mut tx = pool.begin().await?;
    sqlx::raw_sql(AssertSqlSafe(sql.to_owned())).execute(&mut *tx).await?;
    tx.commit().await
}

async fn succeeds(pool: &PgPool, sql: &str) {
    if let Err(error) = execute(pool, sql).await {
        let db_error = error.as_database_error();
        panic!(
            "应提交成功，SQLSTATE={:?}，约束={:?}",
            db_error.and_then(sqlx::error::DatabaseError::code),
            db_error.and_then(sqlx::error::DatabaseError::constraint),
        );
    }
}

async fn fails(pool: &PgPool, sql: &str, state: &str, constraint: Option<&str>) {
    let error = execute(pool, sql).await.expect_err("语句或提交应失败");
    let db_error = error.as_database_error().expect("期望服务端 SQLSTATE");
    assert_eq!(db_error.code().as_deref(), Some(state), "约束 {:?}", db_error.constraint());
    if let Some(constraint) = constraint {
        assert_eq!(db_error.constraint(), Some(constraint));
    }
}

async fn check(pool: &PgPool, sql: &str, constraint: &str) {
    fails(pool, sql, "23514", Some(constraint)).await;
}

async fn checks(pool: &PgPool, valid_sql: &str, cases: &[(&str, &str, &str)]) {
    for (valid_part, invalid_part, constraint) in cases {
        assert!(valid_sql.contains(valid_part), "夹具缺少待替换片段");
        check(pool, &valid_sql.replacen(valid_part, invalid_part, 1), constraint).await;
    }
}

fn replay_key(replay_key_id: &str, write_current: bool) -> String {
    format!(
        "INSERT INTO public.consignment_replay_keys
            (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
         VALUES ('{replay_key_id}', {write_current}, decode(repeat('11', 48), 'hex'),
                 decode(repeat('22', 12), 'hex'), 'fake-data-key', '{START}', '{START}');"
    )
}

async fn seed_users_and_key(pool: &PgPool) {
    fixtures::seed_user(pool, USER_1.parse().expect("测试用户 UUID"), "FakeSellerOne").await;
    fixtures::seed_user(pool, USER_2.parse().expect("测试用户 UUID"), "FakeSellerTwo").await;
    succeeds(pool, &replay_key(REPLAY_1, true)).await;
}

fn new_listing(
    listing_id: &str,
    seller_user_id: &str,
    world: &str,
    eid: &str,
    mode: &str,
) -> String {
    let fee_bps = if mode == "direct" { 0 } else { 500 };
    format!(
        "INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
             created_at, updated_at, expires_at)
         VALUES ('{listing_id}', '{seller_user_id}', '{world}', '{eid}', '{mode}', 'resolving', 101,
                 1, {fee_bps}, 1, 1, 0, '{START}', '{START}', '{END}');"
    )
}

fn registration_command(listing_id: &str, seller_user_id: &str, command_id: &str) -> String {
    format!(
        r#"INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{command_id}', 'public_user', '{seller_user_id}', 'register', 'new',
                 decode(repeat('11', 32), 'hex'), '{REPLAY_1}', '{listing_id}', 202, 'accepted',
                 '{{"command_id":"{command_id}","operation":"register","listing_id":"{listing_id}",
                    "listing_version":1,"state":"resolving","disposition":"registered","price":101}}',
                 '{START}');"#
    )
}

fn registration_facts(listing_id: &str, seller_user_id: &str, command_id: &str) -> String {
    registration_command(listing_id, seller_user_id, command_id)
        + &format!(
            "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{listing_id}', 1, '{command_id}', 'public_user', 'registered', NULL,
                 'resolving', NULL, 101, '{START}');"
        )
}

fn register(
    listing_id: &str,
    seller: &str,
    world: &str,
    eid: &str,
    mode: &str,
    command_id: &str,
) -> String {
    new_listing(listing_id, seller, world, eid, mode)
        + &registration_facts(listing_id, seller, command_id)
}

fn snapshot(listing_id: &str, snapshot_id: &str, world: &str, eid: &str) -> String {
    format!(
        r#"INSERT INTO public.consignment_snapshots
            (id, listing_id, generation, adapter, observed_at, digest, world_id,
             external_equipment_id, name, category, alt_name, level_kind, level_number,
             tradeable, owner_name, stat_groups)
         VALUES ('{snapshot_id}', '{listing_id}', 1, 'hvdata-equip-v1', '{LATER}',
                 decode(repeat('11', 32), 'hex'), '{world}', '{eid}', 'Fake Gear', 'Weapon',
                 NULL, 'assigned', 100, true, 'FakeSellerOne',
                 '[{{"name":"Attack","stats":[{{"name":"Damage","value":5,"base":null}}]}}]');"#
    )
}

fn transition(
    listing_id: &str,
    command_id: &str,
    state: &str,
    event_kind: &str,
    reason: &str,
) -> String {
    let (actor_kind, actor_ref, operation) = match event_kind {
        "closed" if reason == "'auction_lot_created'" => ("system", "fake-system", "auto_close"),
        "closed" => ("public_user", USER_1, "withdraw"),
        "repriced" => ("public_user", USER_1, "reprice"),
        "expired" => ("system", "fake-system", "auto_close"),
        _ => ("worker", "fake-worker", "apply_lookup"),
    };
    let internal_fields = if matches!(operation, "apply_lookup" | "auto_close") {
        let json_reason = if reason == "NULL" {
            "null".to_owned()
        } else {
            format!("\"{}\"", reason.trim_matches('\''))
        };
        format!(",\"expires_at\":\"{END}\",\"reason_code\":{json_reason}")
    } else {
        String::new()
    };
    let occurred_at = if event_kind == "expired" { END } else { LATER };
    format!(
        r#"INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{command_id}', '{actor_kind}', '{actor_ref}', '{operation}', '{listing_id}',
                 decode(repeat('22', 32), 'hex'), NULL, '{listing_id}', 200, 'ok',
                 '{{"command_id":"{command_id}","operation":"{operation}","listing_id":"{listing_id}",
                    "listing_version":2,"state":"{state}","disposition":"{event_kind}","price":101{internal_fields}}}',
                 '{occurred_at}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{listing_id}', 2, '{command_id}', '{actor_kind}', '{event_kind}', 'resolving',
                 '{state}', {reason}, 101, '{occurred_at}');"#
    )
}

fn publish(listing_id: &str, snapshot_id: &str, command_id: &str, state: &str) -> String {
    format!(
        "UPDATE public.consignment_listings SET state = '{state}', state_version = 2,
                current_snapshot_id = '{snapshot_id}', updated_at = '{LATER}', expires_at = '{END}'
         WHERE id = '{listing_id}';"
    ) + &transition(listing_id, command_id, state, "published", "NULL")
}

fn queued_work() -> String {
    format!(
        "INSERT INTO public.consignment_lookup_work
            (id, listing_id, generation, state, attempt_count, fence, next_attempt_at, lease_owner,
             token, lease_until, last_reason, completed_at, created_at, updated_at)
         VALUES ('{WORK}', '{LISTING_1}', 1, 'queued', 0, 0, '{LATER}', NULL,
                 NULL, NULL, NULL, NULL, '{START}', '{START}');"
    )
}

fn envelope() -> String {
    format!(
        "INSERT INTO public.consignment_registration_secrets
            (listing_id, aad_equipment_id, ciphertext, nonce, encryption_key_id, fingerprint,
             created_at, updated_at, retired_at)
         VALUES ('{LISTING_1}', '91000000-0000-0000-0000-000000000001', decode('11', 'hex'),
                 decode(repeat('22', 12), 'hex'), 'fake-data-key', repeat('a', 64), '{START}', '{START}', NULL);"
    )
}

fn baseline(lot_id: &str) -> String {
    format!(
        "INSERT INTO public.consignment_listing_lot_baselines (listing_id, lot_id)
         VALUES ('{LISTING_1}', '{lot_id}');"
    )
}

fn conflict(lot_id: &str) -> String {
    format!(
        "INSERT INTO public.consignment_listing_auction_conflicts (listing_id, lot_id, detected_at, command_id)
         VALUES ('{LISTING_1}', '{lot_id}', '{LATER}', '{LOOKUP_1}');"
    )
}

async fn seed_old_lots(pool: &PgPool) {
    let deadline = auction_time::Timestamp::now() + auction_time::Duration::hours(6);
    let auction_seed = sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ('80000000-0000-0000-0000-000000000001', 'main', 1, 'Fake Main',
                 'live', $1::timestamptz - interval '2 hours', $1 - interval '3 hours', $1, $1, $2),
                ('80000000-0000-0000-0000-000000000002', 'isekai', 1, 'Fake Isekai',
                 'live', $1::timestamptz - interval '2 hours', $1 - interval '3 hours', $1, $1, $2)",
    )
    .bind(deadline)
    .bind(fixtures::policy_snapshot(deadline, 50_000, 30_000))
    .execute(pool)
    .await
    .map(|result| result.rows_affected());
    assert_eq!(auction_seed.ok(), Some(2), "写入两个世界的测试场次");
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.equipment (id, world_id, external_equipment_id, name, category, collected_at)
             VALUES ('70000000-0000-0000-0000-000000000001', 'main', repeat('0', 62) || '42', 'Fake A', 'Weapon', '{START}'),
                    ('70000000-0000-0000-0000-000000000002', 'main', '00042', 'Fake B', 'Weapon', '{START}'),
                    ('70000000-0000-0000-0000-000000000003', 'main', '43', 'Fake C', 'Weapon', '{START}'),
                    ('70000000-0000-0000-0000-000000000004', 'isekai', '00042', 'Fake D', 'Weapon', '{START}');
             INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price, category_key, category_number)
             VALUES ('{LOT_1}', '80000000-0000-0000-0000-000000000001',
                    '70000000-0000-0000-0000-000000000001', 1, 'unsold', 101, 'Weapon', 1),
                    ('{LOT_2}', '80000000-0000-0000-0000-000000000001',
                    '70000000-0000-0000-0000-000000000002', 2, 'unsold', 101, 'Weapon', 2),
                    ('{WRONG_EID_LOT}', '80000000-0000-0000-0000-000000000001',
                    '70000000-0000-0000-0000-000000000003', 3, 'unsold', 101, 'Weapon', 3),
                    ('{WRONG_WORLD_LOT}', '80000000-0000-0000-0000-000000000002',
                    '70000000-0000-0000-0000-000000000004', 1, 'unsold', 101, 'Weapon', 1);"
        ),
    )
    .await;
}

#[tokio::test]
async fn consignment_alias_uses_ascii_without_numeric_conversion() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let padded = format!("{}42", "0".repeat(4096));
    let long_positive = "9".repeat(4096);
    for (input, expected) in [
        ("42", Some("42")),
        ("00042", Some("42")),
        (padded.as_str(), Some("42")),
        (long_positive.as_str(), Some(long_positive.as_str())),
        ("", None),
        ("0000", None),
        ("-42", None),
        ("+42", None),
        (" 42", None),
        ("42\n", None),
        ("４２", None),
        ("42.0", None),
    ] {
        let observed: Option<String> =
            sqlx::query_scalar("SELECT public.consignment_eid_alias($1)")
                .bind(input)
                .fetch_one(pool)
                .await
                .expect("规范化旧装备编号");
        assert_eq!(observed.as_deref(), expected);
    }
    let sql_null: Option<String> = sqlx::query_scalar("SELECT public.consignment_eid_alias(NULL)")
        .fetch_one(pool)
        .await
        .expect("规范化 SQL NULL");
    assert_eq!(sql_null, None);
}

#[tokio::test]
async fn consignment_listings_reject_bad_identity_amount_versions_and_state() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    let registration = register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1);
    checks(
        pool,
        &registration,
        &[
            (LISTING_1, NIL, "consignment_listing_id"),
            (USER_1, NIL, "consignment_listing_seller_user_id"),
            ("'main'", "'other'", "consignment_listing_world_id"),
            ("'42'", "'00042'", "consignment_listing_external_equipment_id"),
            ("'42'", "repeat('4', 33)", "consignment_listing_external_equipment_id"),
            ("'direct'", "'unknown'", "consignment_listing_mode"),
            ("'resolving', 101", "'resolving', 0", "consignment_listing_price"),
            ("'resolving', 101", "'resolving', 1000000001", "consignment_listing_price"),
            ("1, 0, 1, 1, 0", "2, 0, 1, 1, 0", "consignment_listing_pricing_version"),
            ("1, 0, 1, 1, 0", "1, 1, 1, 1, 0", "consignment_listing_fee_bps"),
            ("1, 0, 1, 1, 0", "1, 0, 1, 0, 0", "consignment_listing_service_epoch"),
            ("1, 0, 1, 1, 0", "1, 0, 1, 1, -1", "consignment_listing_blacklist_count_at_register"),
            (START, "infinity", "consignment_listing_created_at"),
            (END, START, "consignment_listing_expires_at"),
        ],
    )
    .await;
    check(
        pool,
        &registration.replacen("1, 0, 1, 1, 0", "1, 0, 2, 1, 0", 1),
        "consignment_listing_transition",
    )
    .await;
    succeeds(pool, &registration).await;

    let consigned = register(LISTING_2, USER_2, "isekai", "42", "consigned", REGISTER_2);
    for replacement in ["1, -1, 1, 1, 0", "1, 10000, 1, 1, 0"] {
        check(
            pool,
            &consigned.replacen("1, 500, 1, 1, 0", replacement, 1),
            "consignment_listing_fee_bps",
        )
        .await;
    }
    succeeds(pool, &consigned).await;
    let own_snapshot = snapshot(LISTING_1, SNAPSHOT_1, "main", "42");
    succeeds(pool, &own_snapshot).await;
    check(
        pool,
        &publish(LISTING_1, SNAPSHOT_1, LOOKUP_1, "awaiting_deposit"),
        "consignment_listing_state_shape",
    )
    .await;
    check(
        pool,
        &publish(LISTING_1, SNAPSHOT_1, LOOKUP_1, "displayed")
            .replace(&format!("'{SNAPSHOT_1}'"), "NULL"),
        "consignment_listing_state_shape",
    )
    .await;
    succeeds(pool, &publish(LISTING_1, SNAPSHOT_1, LOOKUP_1, "displayed")).await;

    let lookup_failed = format!(
        "UPDATE public.consignment_listings SET state='rejected', state_version=2, updated_at='{LATER}',
                 reason_code='lookup_exhausted' WHERE id='{LISTING_2}';"
    ) + &transition(LISTING_2, LOOKUP_2, "rejected", "rejected", "'lookup_exhausted'");
    check(
        pool,
        &lookup_failed.replace("lookup_exhausted", "unknown_reason"),
        "consignment_listing_state_shape",
    )
    .await;
    check(
        pool,
        &lookup_failed.replace("lookup_exhausted", "owner_mismatch"),
        "consignment_listing_state_shape",
    )
    .await;
    succeeds(pool, &lookup_failed).await;
    check(pool, &format!("UPDATE public.consignment_listings SET price=102, state_version=3 WHERE id='{LISTING_2}';"),
          "consignment_listing_transition").await;
}

#[tokio::test]
async fn consignment_activity_and_publication_have_distinct_unique_slots() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    succeeds(pool, &register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1)).await;
    fails(
        pool,
        &register(LISTING_2, USER_1, "main", "42", "consigned", REGISTER_2),
        "23505",
        Some("consignment_listings_seller_active_idx"),
    )
    .await;
    succeeds(pool, &register(LISTING_2, USER_2, "main", "42", "consigned", REGISTER_2)).await;
    succeeds(pool, &snapshot(LISTING_1, SNAPSHOT_1, "main", "42")).await;
    succeeds(pool, &snapshot(LISTING_2, SNAPSHOT_2, "main", "42")).await;
    succeeds(pool, &publish(LISTING_1, SNAPSHOT_1, LOOKUP_1, "displayed")).await;
    fails(
        pool,
        &publish(LISTING_2, SNAPSHOT_2, LOOKUP_2, "awaiting_deposit"),
        "23505",
        Some("consignment_listings_equipment_published_idx"),
    )
    .await;
    let close = format!(
        "UPDATE public.consignment_listings SET state='closed', state_version=3, updated_at='{LATER}',
                 reason_code='seller_cancelled' WHERE id='{LISTING_1}';"
    ) + &transition(LISTING_1, WITHDRAW, "closed", "closed", "'seller_cancelled'")
        .replace("listing_version\":2", "listing_version\":3")
        .replace(&format!("'{LISTING_1}', 2,"), &format!("'{LISTING_1}', 3,"))
        .replace("'closed', 'resolving'", "'closed', 'displayed'");
    succeeds(pool, &close).await;
    succeeds(pool, &publish(LISTING_2, SNAPSHOT_2, LOOKUP_2, "awaiting_deposit")).await;
    fails(
        pool,
        &register(LISTING_3, USER_2, "main", "42", "direct", REGISTER_3),
        "23505",
        Some("consignment_listings_seller_active_idx"),
    )
    .await;
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_listings WHERE state IN ('displayed', 'awaiting_deposit')",
    ).fetch_one(pool).await.expect("读取公开占用");
    assert_eq!(active, 1);
}

#[tokio::test]
async fn consignment_snapshots_limit_fields_and_bind_listing_identity() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    succeeds(pool, &register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1)).await;
    succeeds(pool, &register(LISTING_2, USER_2, "isekai", "42", "consigned", REGISTER_2)).await;
    let row = snapshot(LISTING_1, SNAPSHOT_1, "main", "42");
    checks(
        pool,
        &row,
        &[
            (SNAPSHOT_1, NIL, "consignment_snapshot_id"),
            ("1, 'hvdata", "0, 'hvdata", "consignment_snapshot_generation"),
            ("hvdata-equip-v1", "unknown-adapter", "consignment_snapshot_adapter"),
            (LATER, "infinity", "consignment_snapshot_observed_at"),
            ("repeat('11', 32)", "repeat('11', 31)", "consignment_snapshot_digest"),
            ("'Fake Gear'", "repeat('汉', 171)", "consignment_snapshot_name"),
            ("'Weapon'", "' Weapon'", "consignment_snapshot_category"),
            ("NULL, 'assigned'", "E'Fake\\nName', 'assigned'", "consignment_snapshot_alt_name"),
            ("'FakeSellerOne'", "repeat('a', 257)", "consignment_snapshot_owner_name"),
            ("'assigned', 100, true", "'assigned', NULL, true", "consignment_snapshot_level"),
            ("'assigned', 100, true", "'assigned', 100001, true", "consignment_snapshot_level"),
            ("'assigned', 100, true", "'soulbound', NULL, true", "consignment_snapshot_level"),
            ("'assigned', 100, true", "'unassigned', 100, true", "consignment_snapshot_level"),
            ("'assigned', 100, true", "'unknown', 100, true", "consignment_snapshot_level"),
            ("\"value\":5", "\"value\":1000000001", "consignment_snapshot_stat_groups"),
            ("\"value\":5", "\"value\":\"5\"", "consignment_snapshot_stat_groups"),
            (
                "\"base\":null",
                "\"base\":null,\"unexpected\":\"fake-secret\"",
                "consignment_snapshot_stat_groups",
            ),
            ("\"base\":null", "\"base\":-1000000001", "consignment_snapshot_stat_groups"),
            ("\"name\":\"Attack\"", "\"name\":\" Attack \"", "consignment_snapshot_stat_groups"),
            ("\"name\":\"Damage\"", "\"name\":\" \"", "consignment_snapshot_stat_groups"),
        ],
    )
    .await;
    check(pool, &snapshot(LISTING_1, SNAPSHOT_1, "isekai", "42"), "consignment_snapshot_identity")
        .await;
    check(pool, &snapshot(LISTING_1, SNAPSHOT_1, "main", "43"), "consignment_snapshot_identity")
        .await;
    succeeds(pool, &row).await;
    fails(
        pool,
        &snapshot(LISTING_1, SNAPSHOT_2, "main", "42"),
        "23505",
        Some("consignment_snapshots_listing_id_generation_key"),
    )
    .await;
    succeeds(pool, &snapshot(LISTING_2, SNAPSHOT_2, "isekai", "42")).await;
    fails(
        pool,
        &publish(LISTING_1, SNAPSHOT_2, LOOKUP_1, "displayed"),
        "23503",
        Some("consignment_listing_current_snapshot_fk"),
    )
    .await;
    let next_snapshot_id = "30000000-0000-0000-0000-000000000003";
    let soulbound = snapshot(LISTING_1, next_snapshot_id, "main", "42")
        .replace("1, 'hvdata", "2, 'hvdata")
        .replace("'assigned', 100, true", "'soulbound', NULL, false");
    check(
        pool,
        &(soulbound + &publish(LISTING_1, next_snapshot_id, LOOKUP_1, "displayed")),
        "consignment_listing_version_event",
    )
    .await;
    // 当前快照可以在同一事务后写入，提交时须已属于本单。
    let update =
        publish(LISTING_1, SNAPSHOT_1, LOOKUP_1, "displayed").replace(SNAPSHOT_1, next_snapshot_id);
    let next_snapshot = snapshot(LISTING_1, next_snapshot_id, "main", "42")
        .replace("1, 'hvdata", "2, 'hvdata")
        .replace("'assigned', 100, true", "'unassigned', NULL, true");
    succeeds(pool, &(update + &next_snapshot)).await;
}

#[tokio::test]
async fn consignment_baselines_freeze_at_registration_and_lot_links_are_disjoint() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    seed_old_lots(pool).await;
    let registration = register(LISTING_1, USER_1, "main", "42", "consigned", REGISTER_1);
    for lot_id in [WRONG_EID_LOT, WRONG_WORLD_LOT] {
        check(pool, &(registration.clone() + &baseline(lot_id)), "consignment_lot_identity").await;
    }
    check(pool, &(registration.clone() + &baseline(NIL)), "consignment_baseline_lot_id").await;
    let uncommitted_lot = "60000000-0000-0000-0000-000000000005";
    let new_equipment = "70000000-0000-0000-0000-000000000005";
    let create_lot = format!(
        "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, collected_at)
         VALUES ('{new_equipment}', 'main', '000042', 'Fake E', 'Weapon', '{START}');
         INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price, category_key, category_number)
         SELECT '{uncommitted_lot}', auction_id, '{new_equipment}', 4, 'unsold', 101, category_key, 4
         FROM public.lots WHERE id='{LOT_1}';"
    );
    check(
        pool,
        &(create_lot + &registration + &baseline(uncommitted_lot)),
        "consignment_lot_identity",
    )
    .await;
    succeeds(pool, &(registration + &baseline(LOT_1))).await;
    // 使用相同的夹具时刻仍不能在另一事务为历史单追加基线。
    check(pool, &baseline(LOT_2), "consignment_baseline_registration").await;
    let close = format!(
        "UPDATE public.consignment_listings SET state='closed', state_version=2, updated_at='{LATER}',
                 reason_code='auction_lot_created' WHERE id='{LISTING_1}';"
    ) + &transition(LISTING_1, LOOKUP_1, "closed", "closed", "'auction_lot_created'");
    succeeds(pool, &close).await;
    for lot_id in [WRONG_EID_LOT, WRONG_WORLD_LOT] {
        check(pool, &conflict(lot_id), "consignment_lot_identity").await;
    }
    check(pool, &conflict(LOT_1), "consignment_lot_baseline_disjoint").await;
    check(pool, &conflict(NIL), "consignment_conflict_lot_id").await;
    check(pool, &conflict(LOT_2).replace(LATER, "infinity"), "consignment_conflict_detected_at")
        .await;
    succeeds(pool, &conflict(LOT_2)).await;
    fails(pool, &conflict(LOT_2), "23505", Some("consignment_listing_auction_conflicts_pkey"))
        .await;
}

#[tokio::test]
async fn consignment_commands_and_events_require_the_final_same_version_facts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    let registration = register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1);
    checks(
        pool,
        &registration,
        &[
            ("'public_user',", "'admin',", "consignment_command_actor"),
            ("'register', 'new'", "'register', 'other'", "consignment_command_target"),
            ("'accepted'", "'ok'", "consignment_command_result_code"),
            ("repeat('11', 32)", "repeat('11', 31)", "consignment_command_request_digest"),
            (
                "\"disposition\":\"registered\"",
                "\"disposition\":\"unchanged\"",
                "consignment_command_receipt",
            ),
            (
                "\"price\":101",
                "\"price\":101,\"unexpected\":\"fake-secret\"",
                "consignment_command_receipt",
            ),
            ("\"price\":101", "\"price\":\"101\"", "consignment_command_receipt"),
            ("'registered', NULL", "'registered', 'resolving'", "consignment_event_edge"),
            ("NULL, 101,", "NULL, 0,", "consignment_event_price"),
            (
                "NULL, 101, '2026-01-01T00:00:00Z'",
                "NULL, 101, 'infinity'",
                "consignment_event_occurred_at",
            ),
        ],
    )
    .await;
    check(pool, &registration.replace(REGISTER_1, NIL), "consignment_command_command_id").await;
    check(
        pool,
        &new_listing(LISTING_1, USER_1, "main", "42", "direct"),
        "consignment_listing_version_event",
    )
    .await;
    succeeds(pool, &registration).await;
    let duplicate = register(LISTING_2, USER_2, "main", "43", "consigned", REGISTER_1);
    fails(pool, &duplicate, "23505", Some("consignment_commands_pkey")).await;
    fails(pool, &format!(
        "INSERT INTO public.consignment_events SELECT * FROM public.consignment_events WHERE listing_id='{LISTING_1}';"
    ), "23505", Some("consignment_events_pkey")).await;
    check(pool, &format!(
        "UPDATE public.consignment_listings SET price=102, state_version=2 WHERE id='{LISTING_1}';"
    ), "consignment_listing_version_event").await;
    let expired = transition(LISTING_1, LOOKUP_1, "expired", "expired", "'lookup_deadline'");
    check(pool, &expired, "consignment_event_has_version").await;
    check(pool, &expired.replace("lookup_deadline", "display_expired"), "consignment_event_edge")
        .await;
    let expire_update = format!(
        "UPDATE public.consignment_listings SET state='expired', state_version=2, updated_at='{END}',
                 reason_code='lookup_deadline' WHERE id='{LISTING_1}';"
    );
    check(pool, &(expire_update.replace(END, LATER) + &expired), "consignment_listing_transition")
        .await;
    succeeds(pool, &(expire_update + &expired)).await;
}

#[tokio::test]
async fn consignment_envelopes_validate_shape_and_cannot_be_restored_after_retirement() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    succeeds(pool, &register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1)).await;
    let row = envelope();
    checks(
        pool,
        &row,
        &[
            (
                "91000000-0000-0000-0000-000000000001",
                NIL,
                "consignment_registration_aad_equipment_id",
            ),
            ("decode('11', 'hex')", "NULL", "consignment_registration_envelope"),
            ("decode('11', 'hex')", "decode('', 'hex')", "consignment_registration_envelope"),
            ("repeat('22', 12)", "repeat('22', 11)", "consignment_registration_envelope"),
            ("fake-data-key", "-fake", "consignment_registration_envelope"),
            ("repeat('a', 64)", "repeat('A', 64)", "consignment_registration_envelope"),
            (START, "infinity", "consignment_registration_created_at"),
        ],
    )
    .await;
    succeeds(pool, &row).await;
    check(
        pool,
        "UPDATE public.consignment_registration_secrets SET retired_at = updated_at",
        "consignment_registration_envelope",
    )
    .await;
    succeeds(pool, &format!(
        "UPDATE public.consignment_registration_secrets SET ciphertext=NULL, nonce=NULL, encryption_key_id=NULL,
                fingerprint=NULL, retired_at='{LATER}', updated_at='{LATER}'"
    )).await;
    check(
        pool,
        "UPDATE public.consignment_registration_secrets SET retired_at=NULL, ciphertext=decode('11', 'hex'),
                nonce=decode(repeat('22', 12), 'hex'), encryption_key_id='fake-data-key', fingerprint=repeat('a', 64)",
        "consignment_registration_retirement",
    )
    .await;

    let key = replay_key(REPLAY_2, false);
    checks(
        pool,
        &key,
        &[
            (REPLAY_2, NIL, "consignment_replay_key_id"),
            ("repeat('11', 48)", "repeat('11', 47)", "consignment_replay_key_wrapped_key"),
            ("repeat('22', 12)", "repeat('22', 13)", "consignment_replay_key_nonce"),
            ("fake-data-key", " fake-data-key", "consignment_replay_key_encryption_key_id"),
            (START, "infinity", "consignment_replay_key_created_at"),
        ],
    )
    .await;
    fails(
        pool,
        &replay_key(REPLAY_2, true),
        "23505",
        Some("consignment_replay_keys_write_current_idx"),
    )
    .await;
    succeeds(pool, &key).await;
    succeeds(
        pool,
        &format!(
            "UPDATE public.consignment_replay_keys SET write_current=false WHERE id='{REPLAY_1}';
         UPDATE public.consignment_replay_keys SET write_current=true WHERE id='{REPLAY_2}';"
        ),
    )
    .await;
    let current: String = sqlx::query_scalar(
        "SELECT id::text FROM public.consignment_replay_keys WHERE write_current",
    )
    .fetch_one(pool)
    .await
    .expect("读取当前比较密钥标识");
    assert_eq!(current, REPLAY_2);
}

#[tokio::test]
async fn consignment_work_and_global_permit_enforce_complete_durable_shapes() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    succeeds(pool, &register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1)).await;
    let row = queued_work();
    checks(
        pool,
        &row,
        &[
            (WORK, NIL, "consignment_lookup_work_id"),
            ("1, 'queued'", "9007199254740992, 'queued'", "consignment_lookup_work_generation"),
            ("'queued'", "'unknown'", "consignment_lookup_work_state"),
            ("'queued', 0, 0", "'queued', 7, 0", "consignment_lookup_work_attempt_count"),
            ("'queued', 0, 0", "'queued', 0, -1", "consignment_lookup_work_fence"),
            (LATER, "infinity", "consignment_lookup_work_next_attempt_at"),
        ],
    )
    .await;
    check(pool, &row.replace(&format!("'{LATER}'"), "NULL"), "consignment_lookup_work_state").await;
    succeeds(pool, &row).await;
    let lease = format!(
        "UPDATE public.consignment_lookup_work SET state='leased', attempt_count=1, fence=1,
                next_attempt_at=NULL, lease_owner='fake-worker', token='{REGISTER_1}',
                lease_until='{END}', network_deadline='{LATER}' WHERE id='{WORK}';"
    );
    checks(
        pool,
        &lease,
        &[
            ("attempt_count=1", "attempt_count=0", "consignment_lookup_work_state"),
            ("fence=1", "fence=0", "consignment_lookup_work_state"),
            ("'fake-worker'", "NULL", "consignment_lookup_work_state"),
            (&format!("token='{REGISTER_1}'"), "token=NULL", "consignment_lookup_work_state"),
            (REGISTER_1, NIL, "consignment_lookup_work_token"),
            (END, "infinity", "consignment_lookup_work_lease_until"),
            ("next_attempt_at=NULL", "next_attempt_at=now()", "consignment_lookup_work_state"),
        ],
    )
    .await;
    for invalid in ["NULL", "'infinity'", "'-infinity'", &format!("'{AFTER_END}'")] {
        check(
            pool,
            &lease.replace(
                &format!("network_deadline='{LATER}'"),
                &format!("network_deadline={invalid}"),
            ),
            "consignment_lookup_work_state",
        )
        .await;
    }
    check(
        pool,
        &format!("UPDATE public.consignment_lookup_work SET network_deadline='{LATER}'"),
        "consignment_lookup_work_state",
    )
    .await;
    succeeds(pool, &lease).await;
    succeeds(pool, "UPDATE public.consignment_lookup_work SET network_deadline=lease_until").await;
    check(
        pool,
        "UPDATE public.consignment_lookup_work SET last_reason='lookup_exhausted'",
        "consignment_lookup_work_last_reason",
    )
    .await;
    check(
        pool,
        "UPDATE public.consignment_lookup_work SET state='done'",
        "consignment_lookup_work_state",
    )
    .await;

    let initial: (i64, bool, bool, i64) = sqlx::query_as(
        "SELECT enabled_epoch, admitting,
                holder_work_id IS NULL AND cooldown_until IS NULL AND worker_ready_until IS NULL AND scan_after IS NULL,
                (SELECT count(*) FROM public.consignment_lookup_control)
         FROM public.consignment_lookup_control WHERE id=1",
    ).fetch_one(pool).await.expect("读取许可初始化结果");
    assert_eq!(initial, (1, false, true, 1));
    check(
        owner.postgres_pool(),
        "UPDATE public.consignment_lookup_control SET id=2",
        "consignment_lookup_control_id",
    )
    .await;
    let permit = format!(
        "UPDATE public.consignment_lookup_control SET holder_work_id='{WORK}', holder_fence=1,
                holder_token='{REGISTER_1}', permit_until='{END}' WHERE id=1;"
    );
    checks(
        pool,
        &permit,
        &[
            ("holder_fence=1", "holder_fence=0", "consignment_lookup_control_holder_fence"),
            ("holder_fence=1", "holder_fence=NULL", "consignment_lookup_control_holder"),
            (REGISTER_1, NIL, "consignment_lookup_control_holder_token"),
            (END, "infinity", "consignment_lookup_control_permit_until"),
        ],
    )
    .await;
    fails(
        pool,
        &permit.replace(WORK, NIL),
        "23503",
        Some("consignment_lookup_control_holder_work_id_fkey"),
    )
    .await;
    succeeds(pool, &permit).await;
    let done = format!(
        "UPDATE public.consignment_lookup_work SET state='done', lease_owner=NULL, token=NULL,
                lease_until=NULL, network_deadline=NULL, completed_at='{LATER}', last_reason='source_timeout';
         UPDATE public.consignment_lookup_control SET holder_work_id=NULL, holder_fence=NULL,
                holder_token=NULL, permit_until=NULL;"
    );
    check(
        pool,
        &done.replace("network_deadline=NULL", &format!("network_deadline='{LATER}'")),
        "consignment_lookup_work_state",
    )
    .await;
    succeeds(pool, &done).await;
}

const TABLE_PERMISSIONS: &[(&str, &str, bool, &[&str])] = &[
    ("consignment_commands", "command_id", true, &[]),
    ("consignment_events", "listing_id", true, &[]),
    ("consignment_snapshots", "id", true, &[]),
    ("consignment_listing_lot_baselines", "listing_id", true, &[]),
    ("consignment_listing_auction_conflicts", "listing_id", true, &[]),
    (
        "consignment_listings",
        "id",
        true,
        &[
            "state",
            "price",
            "state_version",
            "current_snapshot_id",
            "reason_code",
            "updated_at",
            "expires_at",
        ],
    ),
    (
        "consignment_registration_secrets",
        "listing_id",
        true,
        &["ciphertext", "nonce", "encryption_key_id", "fingerprint", "updated_at", "retired_at"],
    ),
    (
        "consignment_replay_keys",
        "id",
        true,
        &["write_current", "wrapped_key", "nonce", "encryption_key_id", "updated_at"],
    ),
    (
        "consignment_lookup_work",
        "id",
        true,
        &[
            "state",
            "attempt_count",
            "fence",
            "next_attempt_at",
            "lease_owner",
            "token",
            "lease_until",
            "network_deadline",
            "last_reason",
            "completed_at",
            "updated_at",
        ],
    ),
    (
        "consignment_lookup_control",
        "id",
        false,
        &[
            "enabled_epoch",
            "admitting",
            "next_allowed_at",
            "cooldown_until",
            "holder_work_id",
            "holder_fence",
            "holder_token",
            "permit_until",
            "worker_ready_until",
            "scan_after",
        ],
    ),
];

async fn assert_table_permissions(pool: &PgPool) {
    for &(table, primary_column, can_insert, updatable) in TABLE_PERMISSIONS {
        let table_privileges: (bool, bool, bool, bool, bool, bool, bool) = sqlx::query_as(
            "SELECT has_table_privilege($1::text, 'SELECT'), has_table_privilege($1::text, 'INSERT'),
                    has_table_privilege($1::text, 'UPDATE'), has_table_privilege($1::text, 'DELETE'),
                    has_table_privilege($1::text, 'TRUNCATE'), has_table_privilege($1::text, 'REFERENCES'),
                    has_table_privilege($1::text, 'TRIGGER')",
        )
        .bind(format!("public.{table}"))
        .fetch_one(pool)
        .await
        .expect("读取表级授权");
        assert_eq!(
            table_privileges,
            (true, can_insert, false, false, false, false, false),
            "{table}"
        );
        let mut actual: Vec<String> = sqlx::query_scalar(
            "SELECT attname::text FROM pg_attribute
             WHERE attrelid=$1::text::regclass AND attnum>0 AND NOT attisdropped
               AND has_column_privilege(attrelid, attname, 'UPDATE')",
        )
        .bind(format!("public.{table}"))
        .fetch_all(pool)
        .await
        .expect("读取可更新列");
        let mut expected = updatable.to_vec();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected, "{table}");
        succeeds(pool, &format!("SELECT {primary_column} FROM public.{table}")).await;
        for statement in [
            format!("UPDATE public.{table} SET {primary_column}={primary_column} WHERE false"),
            format!("DELETE FROM public.{table} WHERE false"),
            format!("TRUNCATE public.{table} CASCADE"),
        ] {
            fails(pool, &statement, "42501", None).await;
        }
        for column in updatable {
            succeeds(pool, &format!("UPDATE public.{table} SET {column}={column} WHERE false"))
                .await;
        }
    }
    fails(pool, "INSERT INTO public.consignment_lookup_control DEFAULT VALUES", "42501", None)
        .await;
    fails(pool, "CREATE TABLE public.consignment_runtime_probe (id int)", "42501", None).await;
    fails(pool, "SELECT public.apply_auction_runtime_privileges()", "42501", None).await;
}

#[tokio::test]
async fn consignment_runtime_acl_rebuild_preserves_only_the_declared_privileges() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    assert_table_permissions(runtime.postgres_pool()).await;
    // 模拟 --no-acl 恢复后重建授权，并验证意外授予的额外列权限也被收回。
    succeeds(
        owner.postgres_pool(),
        "REVOKE ALL ON ALL TABLES IN SCHEMA public FROM auction_runtime;
         GRANT UPDATE (mode) ON public.consignment_listings TO auction_runtime;
         SELECT public.apply_auction_runtime_privileges();",
    )
    .await;
    assert_table_permissions(runtime.postgres_pool()).await;
}

#[tokio::test]
async fn consignment_history_rejects_owner_update_delete_and_truncate() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    seed_old_lots(pool).await;
    succeeds(
        pool,
        &(register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1) + &baseline(LOT_1)),
    )
    .await;
    succeeds(pool, &snapshot(LISTING_1, SNAPSHOT_1, "main", "42")).await;
    succeeds(pool, &publish(LISTING_1, SNAPSHOT_1, LOOKUP_1, "displayed")).await;
    succeeds(pool, &conflict(LOT_2)).await;
    for &(table, column, _, _) in &TABLE_PERMISSIONS[..5] {
        let row_count: i64 =
            sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM public.{table}")))
                .fetch_one(pool)
                .await
                .expect("确认只追加表已有记录");
        assert!(row_count > 0, "{table}");
        for statement in [
            format!("UPDATE public.{table} SET {column}={column}"),
            format!("DELETE FROM public.{table}"),
            format!("TRUNCATE public.{table} CASCADE"),
        ] {
            fails(owner.postgres_pool(), &statement, "55000", None).await;
        }
    }
}

#[tokio::test]
async fn consignment_optional_fields_and_update_timestamps_fail_closed() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    let listing = new_listing(LISTING_1, USER_1, "main", "42", "direct");
    // 首写触发器会先拒绝越界的登记版本。维护账号在同一回滚事务中单独验证列 CHECK。
    for bad_version in [0, 9_007_199_254_740_992_i64] {
        let invalid = listing
            .replace("1, 0, 1, 1, 0", &format!("1, 0, {bad_version}, 1, 0"))
            .replace(
                "created_at, updated_at, expires_at)",
                "created_at, updated_at, expires_at, registration_xact_id)",
            )
            .replace(&format!("'{END}');"), &format!("'{END}', pg_current_xact_id());"));
        check(
            owner.postgres_pool(),
            &format!(
                "ALTER TABLE public.consignment_listings DISABLE TRIGGER consignment_guard_listing;
             {invalid}"
            ),
            "consignment_listing_state_version",
        )
        .await;
    }
    check(
        pool,
        &listing.replace(&format!("'{START}', '{START}'"), &format!("'{START}', '-infinity'")),
        "consignment_listing_updated_at",
    )
    .await;
    let register = register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1);
    check(
        pool,
        &register.replace(
            &format!("'{START}');INSERT INTO public.consignment_events"),
            "'infinity');INSERT INTO public.consignment_events",
        ),
        "consignment_command_created_at",
    )
    .await;
    succeeds(pool, &register).await;
    check(
        pool,
        &publish(LISTING_1, NIL, LOOKUP_1, "displayed"),
        "consignment_listing_current_snapshot_id",
    )
    .await;
    let event = transition(LISTING_1, LOOKUP_1, "expired", "expired", "'lookup_deadline'");
    check(
        pool,
        &event.replace(&format!("'{LISTING_1}', 2,"), &format!("'{LISTING_1}', 0,")),
        "consignment_event_state_version",
    )
    .await;

    checks(
        pool,
        &snapshot(LISTING_1, SNAPSHOT_1, "main", "42"),
        &[
            ("'main'", "'other'", "consignment_snapshot_world_id"),
            ("'42'", "'0042'", "consignment_snapshot_external_equipment_id"),
            ("'Fake Gear'", "'Fake /equip/URL'", "consignment_snapshot_name"),
        ],
    )
    .await;

    succeeds(pool, &envelope()).await;
    check(
        pool,
        "UPDATE public.consignment_registration_secrets SET updated_at='-infinity'",
        "consignment_registration_updated_at",
    )
    .await;
    check(
        pool,
        "UPDATE public.consignment_registration_secrets SET ciphertext=NULL, nonce=NULL,
           encryption_key_id=NULL, fingerprint=NULL, retired_at='infinity'",
        "consignment_registration_envelope",
    )
    .await;
    check(
        pool,
        "UPDATE public.consignment_registration_secrets SET ciphertext=NULL, nonce=NULL,
           encryption_key_id=NULL, fingerprint=NULL, retired_at='infinity', updated_at='infinity'",
        "consignment_registration_retired_at",
    )
    .await;
    check(
        pool,
        "UPDATE public.consignment_replay_keys SET updated_at='-infinity'",
        "consignment_replay_key_updated_at",
    )
    .await;

    let work = queued_work();
    check(
        pool,
        &work.replace(&format!("'{START}', '{START}'"), &format!("'infinity', '{START}'")),
        "consignment_lookup_work_created_at",
    )
    .await;
    succeeds(pool, &work).await;
    check(
        pool,
        "UPDATE public.consignment_lookup_work SET state='cancelled', next_attempt_at=NULL,
           completed_at='infinity'",
        "consignment_lookup_work_completed_at",
    )
    .await;
    check(
        pool,
        "UPDATE public.consignment_lookup_work SET updated_at='-infinity'",
        "consignment_lookup_work_updated_at",
    )
    .await;
    succeeds(pool, &format!(
        "UPDATE public.consignment_lookup_work SET state='cancelled', next_attempt_at=NULL, completed_at='{LATER}'"
    )).await;

    for (assignment, constraint) in [
        ("enabled_epoch=0", "consignment_lookup_control_enabled_epoch"),
        ("next_allowed_at='infinity'", "consignment_lookup_control_next_allowed_at"),
        ("cooldown_until='infinity'", "consignment_lookup_control_cooldown_until"),
        ("worker_ready_until='infinity'", "consignment_lookup_control_worker_ready_until"),
        (
            "scan_after='00000000-0000-0000-0000-000000000000'",
            "consignment_lookup_control_scan_after",
        ),
    ] {
        check(
            pool,
            &format!("UPDATE public.consignment_lookup_control SET {assignment}"),
            constraint,
        )
        .await;
    }
    succeeds(
        pool,
        &format!(
        "UPDATE public.consignment_lookup_control SET enabled_epoch=2, next_allowed_at='{LATER}',
                cooldown_until='{END}', worker_ready_until='{LATER}', scan_after='{LISTING_1}'"
    ),
    )
    .await;
}

#[tokio::test]
async fn consignment_internal_receipts_preserve_safe_retry_and_collision_results() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    succeeds(pool, &register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1)).await;
    let reschedule = format!(
        r#"INSERT INTO public.consignment_commands
                (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
                 listing_id, result_status, result_code, result_body, created_at)
            VALUES ('{LOOKUP_1}', 'worker', 'fake-worker', 'apply_lookup', '{LISTING_1}',
                    decode(repeat('11', 32), 'hex'), '{LISTING_1}', 200, 'ok',
                    '{{"command_id":"{LOOKUP_1}","operation":"apply_lookup","listing_id":"{LISTING_1}",
                       "listing_version":null,"state":null,"disposition":"rescheduled","price":null,
                       "next_attempt_at":"{LATER}","last_reason":"source_timeout"}}', '{START}');"#
    );
    checks(
        pool,
        &reschedule,
        &[
            (LATER, "2026-02-30T00:00:00Z", "consignment_command_receipt"),
            ("source_timeout", "lookup_exhausted", "consignment_command_receipt"),
            (
                "\"last_reason\":\"source_timeout\"",
                "\"last_reason\":\"source_timeout\",\"token\":\"fake-token\"",
                "consignment_command_receipt",
            ),
        ],
    )
    .await;
    succeeds(pool, &reschedule).await;
    let state_version: i64 =
        sqlx::query_scalar("SELECT state_version FROM public.consignment_listings")
            .fetch_one(pool)
            .await
            .expect("核对解析重排未改变版本");
    assert_eq!(state_version, 1);
    let collision_only = format!(
        r#"INSERT INTO public.consignment_commands
                (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
                 listing_id, result_status, result_code, result_body, created_at)
            VALUES ('{LOOKUP_2}', 'system', 'fake-system', 'auto_close', '{LISTING_1}',
                    decode(repeat('22', 32), 'hex'), '{LISTING_1}', 200, 'ok',
                    '{{"command_id":"{LOOKUP_2}","operation":"auto_close","listing_id":"{LISTING_1}",
                       "listing_version":null,"state":null,"disposition":"conflicts_recorded","price":null}}',
                    '{LATER}');"#
    );
    check(
        pool,
        &collision_only.replace("conflicts_recorded", "unknown-effect"),
        "consignment_command_receipt",
    )
    .await;
    succeeds(pool, &collision_only).await;
}

#[tokio::test]
async fn consignment_registration_can_close_another_listing_for_the_same_equipment() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    succeeds(pool, &register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1)).await;
    let clear_old = format!(
        "UPDATE public.consignment_listings SET state='expired', state_version=2, updated_at='{END}',
                reason_code='lookup_deadline' WHERE id='{LISTING_1}';
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING_1}', 2, '{REGISTER_2}', 'system', 'expired', 'resolving',
                 'expired', 'lookup_deadline', 101, '{END}');"
    );
    for (world, eid) in [("main", "43"), ("isekai", "42")] {
        let new = register(LISTING_2, USER_2, world, eid, "consigned", REGISTER_2);
        check(pool, &(new + &clear_old), "consignment_listing_version_event").await;
    }
    let new = register(LISTING_2, USER_2, "main", "42", "consigned", REGISTER_2);
    succeeds(pool, &(new + &clear_old)).await;
    let result: (String, i64) = sqlx::query_as(
        "SELECT state, state_version FROM public.consignment_listings WHERE id=$1::text::uuid",
    )
    .bind(LISTING_1)
    .fetch_one(pool)
    .await
    .expect("读取由外层命令清退的旧单");
    assert_eq!(result, ("expired".to_owned(), 2));
}

async fn assert_resolving_deadline(make_change: impl Fn(&str) -> String, snapshot_needed: bool) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    succeeds(pool, &register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1)).await;
    if snapshot_needed {
        succeeds(pool, &snapshot(LISTING_1, SNAPSHOT_1, "main", "42")).await;
    }
    check(pool, &make_change(END), "consignment_listing_transition").await;
    succeeds(pool, &make_change(BEFORE_END)).await;
}

#[tokio::test]
async fn consignment_publish_rejects_the_old_deadline_even_with_a_fresh_expiry() {
    assert_resolving_deadline(
        |at| {
            publish(LISTING_1, SNAPSHOT_1, LOOKUP_1, "displayed")
                .replace(END, PUBLISHED_END)
                .replace(LATER, at)
        },
        true,
    )
    .await;
}

#[tokio::test]
async fn consignment_reprice_rejects_the_old_deadline() {
    assert_resolving_deadline(
        |at| {
            format!(
                "UPDATE public.consignment_listings SET price=102, state_version=2, updated_at='{at}'
                 WHERE id='{LISTING_1}';"
            ) + &transition(LISTING_1, WITHDRAW, "resolving", "repriced", "NULL")
                .replace("\"price\":101", "\"price\":102")
                .replace("NULL, 101,", "NULL, 102,")
                .replace(LATER, at)
        },
        false,
    )
    .await;
}

#[tokio::test]
async fn consignment_withdraw_rejects_the_old_deadline() {
    assert_resolving_deadline(
        |at| {
            format!(
                "UPDATE public.consignment_listings SET state='closed', reason_code='seller_cancelled',
                        state_version=2, updated_at='{at}' WHERE id='{LISTING_1}';"
            ) + &transition(LISTING_1, WITHDRAW, "closed", "closed", "'seller_cancelled'")
                .replace(LATER, at)
        },
        false,
    )
    .await;
}

#[tokio::test]
async fn consignment_system_collision_can_close_after_the_deadline() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    seed_old_lots(pool).await;
    succeeds(pool, &register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1)).await;
    let close = format!(
        "UPDATE public.consignment_listings SET state='closed', reason_code='auction_lot_created',
                state_version=2, updated_at='{AFTER_END}' WHERE id='{LISTING_1}';"
    ) + &transition(LISTING_1, LOOKUP_1, "closed", "closed", "'auction_lot_created'")
        .replace(LATER, AFTER_END)
        + &conflict(LOT_1).replace(LATER, AFTER_END);
    succeeds(pool, &close).await;
}

#[tokio::test]
async fn consignment_published_listings_expire_with_their_mode_reason() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    for (listing_id, register_id, snapshot_id, lookup_id, close_id, eid, mode, state, reason) in [
        (
            LISTING_1,
            REGISTER_1,
            SNAPSHOT_1,
            LOOKUP_1,
            REGISTER_3,
            "42",
            "direct",
            "displayed",
            "display_expired",
        ),
        (
            LISTING_2,
            REGISTER_2,
            SNAPSHOT_2,
            LOOKUP_2,
            WITHDRAW,
            "43",
            "consigned",
            "awaiting_deposit",
            "deposit_expired",
        ),
    ] {
        succeeds(pool, &register(listing_id, USER_1, "main", eid, mode, register_id)).await;
        succeeds(pool, &snapshot(listing_id, snapshot_id, "main", eid)).await;
        succeeds(pool, &publish(listing_id, snapshot_id, lookup_id, state)).await;
        let expire =
            format!(
                "UPDATE public.consignment_listings SET state='expired', reason_code='{reason}',
                    state_version=3, updated_at='{END}' WHERE id='{listing_id}';"
            ) + &transition(listing_id, close_id, "expired", "expired", &format!("'{reason}'"))
                .replace("\"listing_version\":2", "\"listing_version\":3")
                .replace(&format!("'{listing_id}', 2,"), &format!("'{listing_id}', 3,"))
                .replace("'expired', 'resolving'", &format!("'expired', '{state}'"));
        check(pool, &expire.replacen(END, BEFORE_END, 1), "consignment_listing_transition").await;
        check(pool, &expire.replace(reason, "lookup_deadline"), "consignment_event_edge").await;
        succeeds(pool, &expire).await;
        let preserved: bool = sqlx::query_scalar(
            "SELECT current_snapshot_id IS NOT NULL AND updated_at = expires_at
             FROM public.consignment_listings WHERE id=$1::text::uuid",
        )
        .bind(listing_id)
        .fetch_one(pool)
        .await
        .expect("核对到期后的快照与截止");
        assert!(preserved);
    }
}

#[tokio::test]
async fn consignment_price_and_fee_endpoints_are_inclusive() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    let amounts = |listing, command, eid, price, fee| {
        register(listing, USER_1, "main", eid, "consigned", command)
            .replace("'resolving', 101,", &format!("'resolving', {price},"))
            .replace("\"price\":101", &format!("\"price\":{price}"))
            .replace("NULL, 101,", &format!("NULL, {price},"))
            .replace("1, 500, 1, 1, 0", &format!("1, {fee}, 1, 1, 0"))
    };
    for bad_price in [0, 1_000_000_001] {
        check(
            pool,
            &amounts(LISTING_1, REGISTER_1, "42", bad_price, 500),
            "consignment_listing_price",
        )
        .await;
    }
    for bad_fee in [-1, 10_000] {
        check(
            pool,
            &amounts(LISTING_1, REGISTER_1, "42", 101, bad_fee),
            "consignment_listing_fee_bps",
        )
        .await;
    }
    succeeds(pool, &amounts(LISTING_1, REGISTER_1, "42", 1, 0)).await;
    succeeds(pool, &amounts(LISTING_2, REGISTER_2, "43", 1_000_000_000, 9999)).await;
}

fn rejection_command(command_id: &str, operation: &str, status: u16, code: &str) -> String {
    let (actor, actor_ref) = match operation {
        "admin_delist" => ("admin", "fake-admin"),
        "apply_lookup" => ("worker", "fake-worker"),
        "auto_close" => ("system", "fake-system"),
        _ => ("public_user", USER_1),
    };
    let (target, replay_key) = if operation == "register" {
        ("new", format!("'{REPLAY_1}'"))
    } else {
        (LISTING_1, "NULL".to_owned())
    };
    format!(
        r#"INSERT INTO public.consignment_commands
                (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
                 replay_key_id, listing_id, result_status, result_code, result_body, created_at)
            VALUES ('{command_id}', '{actor}', '{actor_ref}', '{operation}', '{target}',
                    decode(repeat('11', 32), 'hex'), {replay_key}, NULL, {status}, '{code}',
                    '{{"command_id":"{command_id}","operation":"{operation}","listing_id":null,
                       "listing_version":null,"state":null,"disposition":"rejected","price":null}}',
                    '{START}');"#
    )
}

#[tokio::test]
async fn consignment_failure_receipts_match_the_operation_status_matrix() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    let cases: &[(u16, &str, &[&str])] = &[
        (403, "participant_ineligible", &["register"]),
        (404, "listing_not_found", &["reprice", "withdraw", "admin_delist"]),
        (409, "already_open", &["register"]),
        (409, "equipment_busy", &["register"]),
        (409, "history_unproven", &["register"]),
        (409, "history_owner_mismatch", &["register"]),
        (409, "history_alias_conflict", &["register"]),
        (409, "fee_changed", &["register"]),
        (409, "world_unavailable", &["register"]),
        (409, "stale_version", &["reprice", "withdraw", "admin_delist"]),
        (409, "listing_not_active", &["reprice", "withdraw", "admin_delist"]),
        (429, "registration_rate_limited", &["register"]),
        (429, "listing_limit_reached", &["register"]),
        (409, "command_id_conflict", &[]),
        (409, "lease_lost", &[]),
        (404, "consignment_disabled", &[]),
        (409, "participant_ineligible", &[]),
        (403, "listing_not_found", &[]),
        (409, "listing_limit_reached", &[]),
    ];
    for (index, (status, code, allowed)) in cases.iter().enumerate() {
        for (actor_index, operation) in
            ["register", "reprice", "withdraw", "admin_delist", "apply_lookup", "auto_close"]
                .into_iter()
                .enumerate()
        {
            let command_id = format!("43000000-0000-0000-0000-{:012}", index * 6 + actor_index + 1);
            let receipt = rejection_command(&command_id, operation, *status, code);
            if allowed.contains(&operation) {
                succeeds(pool, &receipt).await;
            } else {
                check(pool, &receipt, "consignment_command_result_code").await;
            }
        }
    }
}

#[tokio::test]
async fn consignment_success_receipts_cannot_commit_without_their_event() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    succeeds(pool, &register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1)).await;

    check(
        pool,
        &registration_command(LISTING_1, USER_1, REGISTER_2),
        "consignment_command_version_event",
    )
    .await;
    for (state, kind, reason) in [
        ("resolving", "repriced", "NULL"),
        ("closed", "closed", "'seller_cancelled'"),
        ("displayed", "published", "NULL"),
        ("rejected", "rejected", "'lookup_exhausted'"),
        ("expired", "expired", "'lookup_deadline'"),
        ("closed", "closed", "'auction_lot_created'"),
    ] {
        let transition = transition(LISTING_1, LOOKUP_1, state, kind, reason);
        let (receipt, _) =
            transition.split_once("INSERT INTO public.consignment_events").expect("分离命令和事件");
        check(pool, receipt, "consignment_command_version_event").await;
        if kind == "repriced" {
            succeeds(
                pool,
                &receipt
                    .replace(LOOKUP_1, WITHDRAW)
                    .replace("\"repriced\"", "\"unchanged\"")
                    .replace("\"listing_version\":2", "\"listing_version\":1"),
            )
            .await;
        }
    }
    succeeds(pool, &rejection_command(REGISTER_2, "register", 409, "already_open")).await;
}

#[tokio::test]
async fn consignment_command_inserted_under_savepoint_fails_closed() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_users_and_key(pool).await;
    let registration = register(LISTING_1, USER_1, "main", "42", "direct", REGISTER_1);
    let command = registration_command(LISTING_1, USER_1, REGISTER_1);
    let nested = registration.replace(
        &command,
        &format!("SAVEPOINT nested_command; {command} RELEASE SAVEPOINT nested_command;"),
    );
    check(pool, &nested, "consignment_listing_version_event").await;
    succeeds(pool, &registration).await;
}
