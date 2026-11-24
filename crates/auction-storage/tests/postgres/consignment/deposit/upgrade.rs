//! 从 133 升级到 134：旧挂单、命令、事件、投递与入站邮件逐项保留，v1 验证分支不变。

use auction_application::delivery::{DeliveryClaimExclusions, DeliveryWorkStore};
use auction_storage::postgres_migrate_to;
use auction_time::{Duration, Timestamp};
use sqlx::AssertSqlSafe;
use uuid::Uuid;

use super::support::{END, LATER, LISTING, REGISTER, REPLAY, SNAPSHOT, START, USER_1, succeeds};
use crate::support::{TestDatabase, fixtures};

const OLD_MAIL: &str = "70000000-0000-0000-0000-000000000001";
const OLD_SUBMISSION: &str = "70000000-0000-0000-0000-000000000004";
const OLD_TAKE: &str = "70000000-0000-0000-0000-000000000002";
const OLD_TAKE_COMMAND: &str = "70000000-0000-0000-0000-000000000003";
const OLD_AUCTION: &str = "70000000-0000-0000-0000-000000000005";
const OLD_INTAKE_COMMAND: &str = "70000000-0000-0000-0000-000000000006";
const OLD_RETURN_MAIL: &str = "70000000-0000-0000-0000-000000000007";
const OLD_RETURN_ACTIVATE_COMMAND: &str = "70000000-0000-0000-0000-000000000008";

const NEW_TABLES: [&str; 29] = [
    "consignment_season_windows",
    "mail_evidence_sessions",
    "consignment_mail_readiness",
    "mail_originals",
    "mail_original_links",
    "mail_evidence_envelopes",
    "mail_envelope_observations",
    "mail_observation_conflicts",
    "mail_initial_decisions",
    "mail_quarantines",
    "mail_quarantine_rechecks",
    "mail_final_dispositions",
    "mail_quarantine_releases",
    "settlement_recovery_return_handoffs",
    "consignment_deposit_reservations",
    "consignment_reservation_secrets",
    "consignment_equipment_claims",
    "consignment_custody_assets",
    "consignment_deposit_items",
    "consignment_custody_positions",
    "consignment_disposition_generations",
    "consignment_custody_events",
    "consignment_custody_controls",
    "consignment_equipment_handoffs",
    "consignment_season_notices",
    "mail_disposition_work",
    "mail_disposition_attempts",
    "mail_disposition_write_starts",
    "mail_disposition_proofs",
];

async fn row_json(pool: &sqlx::PgPool, query: &str) -> serde_json::Value {
    sqlx::query_scalar(AssertSqlSafe(query.to_owned()))
        .fetch_one(pool)
        .await
        .expect("读取升级前旧行")
}

#[tokio::test]
async fn postgres_consignment_deposit_upgrade_from_133_preserves_old_rows_and_v1_validation() {
    let database = TestDatabase::create_empty().await.expect("创建空库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    postgres_migrate_to(pool, 133).await.expect("迁移到 133");

    succeeds(
        pool,
        &format!(
            "INSERT INTO public.users (id, username, public_key_hash)
             VALUES ('{USER_1}', 'FakeSellerOne', decode(repeat('11', 32), 'hex'));
             INSERT INTO public.consignment_replay_keys
                (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
             VALUES ('{REPLAY}', true, decode(repeat('13', 48), 'hex'),
                     decode(repeat('14', 12), 'hex'), 'fake-data-key', '{START}', '{START}');
             INSERT INTO public.consignment_listings
                (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
                 pricing_version, fee_bps, state_version, service_epoch,
                 blacklist_count_at_register, blacklist_baseline_ids, created_at, updated_at,
                 expires_at)
             VALUES ('{LISTING}', '{USER_1}', 'main', '42', 'direct', 'resolving', 101,
                     1, 0, 1, 1, 0, ARRAY[]::uuid[], '{START}', '{START}', '{END}');
             INSERT INTO public.consignment_commands
                (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
                 replay_key_id, listing_id, result_status, result_code, result_body, created_at)
             VALUES ('{REGISTER}', 'public_user', '{USER_1}', 'register', 'new',
                     decode(repeat('15', 32), 'hex'), '{REPLAY}', '{LISTING}', 202, 'accepted',
                     '{{\"command_id\":\"{REGISTER}\",\"operation\":\"register\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":1,
                        \"state\":\"resolving\",\"disposition\":\"registered\",\"price\":101}}',
                     '{START}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 1, '{REGISTER}', 'public_user', 'registered', NULL,
                     'resolving', NULL, 101, '{START}');
             INSERT INTO public.consignment_snapshots
                (id, listing_id, generation, adapter, observed_at, digest, world_id,
                 external_equipment_id, name, category, alt_name, level_kind, level_number,
                 tradeable, owner_name, stat_groups)
             VALUES ('{SNAPSHOT}', '{LISTING}', 1, 'hvdata-equip-v1', '{LATER}',
                     decode(repeat('16', 32), 'hex'), 'main', '42', 'Fake Gear', 'Weapon',
                     NULL, 'assigned', 100, true, 'FakeSellerOne', '[]'::jsonb);
             INSERT INTO public.inbound_mail
                (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
                 received_at, status, decision_code, decision_metadata, processed_at,
                 sender, subject)
             VALUES ('70000000-0000-0000-0000-000000000001', 'legacy-mailbox', '7001',
                     'main', 'delivery', decode(repeat('17', 32), 'hex'), '{START}',
                     'accepted', 'legacy_code', '{{}}'::jsonb, '{LATER}',
                     'MoogleMail', 'legacy subject');
             INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('70000000-0000-0000-0000-000000000002',
                     '70000000-0000-0000-0000-000000000003',
                     decode(repeat('18', 32), 'hex'), 'manual', 'legacy-origin',
                     'Legacy recipient', '{{}}'::jsonb, 'prepared', 0, 0,
                     '{START}', '{START}');"
        ),
    )
    .await;

    postgres_migrate_to(pool, 134).await.expect("迁移到 134");

    let command: (i16, String, bool, bool) = sqlx::query_as(
        "SELECT schema_version, operation,
                target_kind IS NULL AND target_id IS NULL,
                target_bot_uid IS NULL AND target_season_id IS NULL
         FROM public.consignment_commands WHERE command_id=$1::uuid",
    )
    .bind(REGISTER)
    .fetch_one(pool)
    .await
    .expect("读取旧命令");
    assert_eq!(command, (1, "register".to_owned(), true, true));
    let listing: (String, i64, i64) = sqlx::query_as(
        "SELECT state, state_version, price FROM public.consignment_listings WHERE id=$1::uuid",
    )
    .bind(LISTING)
    .fetch_one(pool)
    .await
    .expect("读取旧挂单");
    assert_eq!(listing, ("resolving".to_owned(), 1, 101));
    let delivery: (String, String) =
        sqlx::query_as("SELECT origin_kind, origin_id FROM public.deliveries WHERE id=$1::uuid")
            .bind("70000000-0000-0000-0000-000000000002")
            .fetch_one(pool)
            .await
            .expect("读取旧投递");
    assert_eq!(delivery, ("manual".to_owned(), "legacy-origin".to_owned()));
    let inbound: (String, String, Option<String>) = sqlx::query_as(
        "SELECT mailbox_key, external_mail_id, sender FROM public.inbound_mail
         WHERE id=$1::uuid",
    )
    .bind("70000000-0000-0000-0000-000000000001")
    .fetch_one(pool)
    .await
    .expect("读取旧入站");
    assert_eq!(
        inbound,
        ("legacy-mailbox".to_owned(), "7001".to_owned(), Some("MoogleMail".to_owned()))
    );
    let has_uid: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema='public' AND table_name='inbound_mail'
              AND column_name IN ('bot_uid','authenticated_bot_uid'))",
    )
    .fetch_one(pool)
    .await
    .expect("检查 inbound_mail 未增加 UID 列");
    assert!(!has_uid);
    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM public.mail_original_links")
        .fetch_one(pool)
        .await
        .expect("检查未回填原信链接");
    assert_eq!(links, 0);

    succeeds(
        pool,
        "INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('71000000-0000-4000-8000-000000000001', 'admin', 'fake-admin',
                 'admin_delist', '72000000-0000-0000-0000-000000000001',
                 decode(repeat('19', 32), 'hex'), NULL, NULL, 404, 'listing_not_found',
                 '{\"command_id\":\"71000000-0000-4000-8000-000000000001\",
                   \"operation\":\"admin_delist\",\"listing_id\":null,
                   \"listing_version\":null,\"state\":null,
                   \"disposition\":\"rejected\",\"price\":null}', '2026-01-01T05:00:00Z');",
    )
    .await;
}

#[tokio::test]
async fn postgres_deposit_upgrade_preserves_inflight_auction_mail() {
    let database = TestDatabase::create_empty().await.expect("创建空库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    postgres_migrate_to(pool, 133).await.expect("迁移到 133");
    let start = START.parse::<Timestamp>().expect("固定时刻");
    let deadline = start + auction_time::Duration::hours(6);
    sqlx::query(
        "INSERT INTO public.users (id, username, public_key_hash)
         VALUES ($1, 'FakeSellerOne', decode(repeat('11', 32), 'hex'))",
    )
    .bind(Uuid::parse_str(USER_1).expect("卖家"))
    .execute(pool)
    .await
    .expect("写入卖家");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1, 'main', 7001, 'Fake upgraded auction', 'settled', $2, $2, $2, $2, $3)",
    )
    .bind(Uuid::parse_str(OLD_AUCTION).expect("场次"))
    .bind(start)
    .bind(fixtures::policy_snapshot(deadline, 50_000, 30_000))
    .execute(pool)
    .await
    .expect("写入旧场次");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'fake_upgrade_intake', $2, decode(repeat('71', 32), 'hex'), 'accepted')",
    )
    .bind(Uuid::parse_str(OLD_INTAKE_COMMAND).expect("命令"))
    .bind(Uuid::parse_str(OLD_AUCTION).expect("场次"))
    .execute(pool)
    .await
    .expect("写入旧投稿命令");
    sqlx::query(
        "INSERT INTO public.auction_submissions
            (id, source_command_id, auction_id, mailbox_key, external_mail_id, seller, received_at)
         VALUES ($1, $2, $3, 'legacy-mailbox', '7001', 'FakeSellerOne', $4)",
    )
    .bind(Uuid::parse_str(OLD_SUBMISSION).expect("投稿"))
    .bind(Uuid::parse_str(OLD_INTAKE_COMMAND).expect("命令"))
    .bind(Uuid::parse_str(OLD_AUCTION).expect("场次"))
    .bind(start)
    .execute(pool)
    .await
    .expect("写入旧投稿");
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at, sender, subject)
         VALUES ($1, 'legacy-mailbox', '7001', 'main', 'equipment',
                 decode(repeat('72', 32), 'hex'), $2, 'accepted', 'legacy_accepted',
                 '{\"equipment_id\":\"fake-equipment\"}'::jsonb, $3, 'FakeSellerOne', 'legacy')",
    )
    .bind(Uuid::parse_str(OLD_MAIL).expect("入站"))
    .bind(start)
    .bind(start + auction_time::Duration::minutes(1))
    .execute(pool)
    .await
    .expect("写入旧入站邮件");
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.inbound_mail
                (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
                 received_at, status, decision_code, processed_at, created_at)
             VALUES ('{OLD_RETURN_MAIL}', 'main:bot', '7011', 'main', 'delivery',
                     decode(repeat('74', 32), 'hex'), '{START}', 'accepted',
                     'settlement_recovery_returned', '{START}', '{START}');
             INSERT INTO public.settlement_return_evidence
                (return_mail_id, match_state, reason_code, recorded_at)
             VALUES ('{OLD_RETURN_MAIL}', 'untrusted', 'source_unproven', '{START}');
             INSERT INTO public.settlement_return_cases
                (return_mail_id, world_id, status, reason_code, resend_limit, created_at,
                 updated_at)
             VALUES ('{OLD_RETURN_MAIL}', 'main', 'quarantined', 'source_unproven', 3,
                     '{START}', '{START}');
             INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, activation_command_id, activation_request_digest, activated_at,
                 fencing_version, attempt_count, created_at, updated_at)
             VALUES ('{OLD_TAKE}', '{OLD_TAKE_COMMAND}', decode(repeat('73', 32), 'hex'),
                     'settlement_return_action', 'settlement_return:{OLD_RETURN_MAIL}',
                     'MoogleMail',
                     '{{\"schema_version\":1,\"operation\":\"take\",\"world\":\"main\",
                       \"external_mail_id\":\"7011\",\"credits\":0,\"cod\":0,
                       \"attachment_ids\":[]}}'::jsonb,
                     'pending', '{OLD_RETURN_ACTIVATE_COMMAND}',
                     decode(repeat('75', 32), 'hex'), '{START}', 0, 0, '{START}', '{START}');
             INSERT INTO public.settlement_return_actions
                (return_mail_id, world_id, source_external_mail_id, expected_credits,
                 expected_cod, expected_attachment_ids, execution_state, delivery_id,
                 prepare_command_id, prepare_request_digest, activate_command_id,
                 activate_request_digest, state_version, created_at, updated_at)
             VALUES ('{OLD_RETURN_MAIL}', 'main', '7011', 0, 0, '{{}}', 'enqueued',
                     '{OLD_TAKE}', '{OLD_TAKE_COMMAND}', decode(repeat('73', 32), 'hex'),
                     '{OLD_RETURN_ACTIVATE_COMMAND}', decode(repeat('75', 32), 'hex'),
                     2, '{START}', '{START}');"
        ),
    )
    .await;
    sqlx::query(
        "INSERT INTO public.mail_poll_state
            (mailbox_key, cursor_value, last_poll_at, next_poll_at, consecutive_failures,
             paused, state_version, updated_at)
         VALUES ('legacy-mailbox', '7001', $1, $1, 0, false, 1, $1)",
    )
    .bind(start)
    .execute(pool)
    .await
    .expect("写入旧 poll 游标");

    let before_inbound = row_json(
        pool,
        &format!("SELECT to_jsonb(m) FROM public.inbound_mail m WHERE id='{OLD_MAIL}'"),
    )
    .await;
    let before_submission = row_json(
        pool,
        &format!(
            "SELECT to_jsonb(s) FROM public.auction_submissions s WHERE id='{OLD_SUBMISSION}'"
        ),
    )
    .await;
    let before_take = row_json(
        pool,
        &format!("SELECT to_jsonb(d) FROM public.deliveries d WHERE id='{OLD_TAKE}'"),
    )
    .await;
    let before_cursor = row_json(
        pool,
        "SELECT to_jsonb(p) FROM public.mail_poll_state p WHERE mailbox_key='legacy-mailbox'",
    )
    .await;

    postgres_migrate_to(pool, 134).await.expect("迁移到 134");

    assert_eq!(
        before_inbound,
        row_json(
            pool,
            &format!("SELECT to_jsonb(m) FROM public.inbound_mail m WHERE id='{OLD_MAIL}'"),
        )
        .await,
        "旧入站邮件必须逐字段不变"
    );
    assert_eq!(
        before_submission,
        row_json(
            pool,
            &format!(
                "SELECT to_jsonb(s) FROM public.auction_submissions s WHERE id='{OLD_SUBMISSION}'"
            ),
        )
        .await,
        "旧投稿必须逐字段不变"
    );
    assert_eq!(
        before_take,
        row_json(
            pool,
            &format!("SELECT to_jsonb(d) FROM public.deliveries d WHERE id='{OLD_TAKE}'"),
        )
        .await,
        "旧 Take 投递必须逐字段不变"
    );
    assert_eq!(
        before_cursor,
        row_json(
            pool,
            "SELECT to_jsonb(p) FROM public.mail_poll_state p WHERE mailbox_key='legacy-mailbox'",
        )
        .await,
        "旧 poll 游标必须逐字段不变"
    );
    let has_uid: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema='public' AND table_name='inbound_mail'
              AND column_name IN ('bot_uid','authenticated_bot_uid'))",
    )
    .fetch_one(pool)
    .await
    .expect("检查入站邮件未加 UID 列");
    assert!(!has_uid);

    let store = database.store().await.expect("运行账号");
    let now = Timestamp::now();
    store.begin_claim_cycle(now, std::time::Duration::from_secs(60)).await.expect("运行领取前清理");
    let claims = store
        .claim_due(
            "upgrade-worker".to_owned(),
            now,
            now + Duration::minutes(2),
            std::time::Duration::from_secs(60),
            DeliveryClaimExclusions::default(),
            10,
        )
        .await
        .expect("通用领取");
    assert!(
        claims.iter().any(|claim| claim.delivery_id == Uuid::parse_str(OLD_TAKE).expect("旧 Take")),
        "旧来源的 pending Take 不得被寄售来源排除误伤"
    );

    let mut new_facts = 0_i64;
    for table in NEW_TABLES {
        let count: i64 =
            sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM public.{table}")))
                .fetch_one(store.postgres_pool())
                .await
                .expect("读取新增表计数");
        new_facts += count;
    }
    assert_eq!(new_facts, 0, "升级与旧领取不得写入任何寄售或新邮件事实");
}
