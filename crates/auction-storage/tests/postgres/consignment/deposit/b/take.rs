//! 4B 取得原语：意愿、止售、额度与季界优先级，以及 position/挂单同事务推进。

use auction_application::consignment::{
    CustodyControlStore, CustodyListingChange, ListingDelistReason, ReleaseOperationsHoldCommand,
    SetOperationsHoldCommand, WithdrawDepositCommand,
};
use auction_domain::UserId;
use auction_domain::consignment::{HeldOffMarketReason, ListingState, ListingVersion};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::support::{
    self, ATTEMPT, DECIDE, DECISION, LISTING, POSITION, PROOF, PROOF_COMMAND, USER_1,
};
use super::super::support::{accepted_envelope, succeeds};
use super::helpers::{client_command, hash, repository, repository_with_max};
use crate::support::TestDatabase;

async fn accepted_database() -> (TestDatabase, PgPool) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    sqlx::query(
        "UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1",
    )
    .execute(pool)
    .await
    .expect("打开寄售闸门");
    (database, pool.clone())
}

async fn accepted_season_database() -> (TestDatabase, PgPool) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    let season = Uuid::new_v4();
    let accepted = support::accepted_envelope()
        .replace("'main'", "'isekai'")
        .replace("|main|", "|isekai|")
        .replace(
            " 'isekai', '42',\n                 NULL, 'awaiting_take'",
            &format!(" 'isekai', '42',\n                 '{season}', 'awaiting_take'"),
        );
    let mut transaction = pool.begin().await.expect("开启季夹具事务");
    sqlx::query(
        "INSERT INTO public.consignment_season_windows
            (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
             calendar_version, preclose_days)
         VALUES ($1, '2026-isekai-old', 'isekai', 'UTC',
                 timestamptz '2025-12-01T00:00:00Z', timestamptz '2026-01-02T00:00:00Z',
                 timestamptz '2025-12-30T00:00:00Z', decode(repeat('aa', 32), 'hex'), 3)",
    )
    .bind(season)
    .execute(&mut *transaction)
    .await
    .expect("写入旧季");
    sqlx::raw_sql(sqlx::AssertSqlSafe(accepted))
        .execute(&mut *transaction)
        .await
        .expect("写入季内已接受夹具");
    transaction.commit().await.expect("提交季夹具");
    sqlx::query(
        "UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1",
    )
    .execute(pool)
    .await
    .expect("打开寄售闸门");
    (database, pool.clone())
}

async fn accepted_preclose_database() -> (TestDatabase, PgPool) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    let season = Uuid::new_v4();
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let accepted = support::accepted_envelope()
        .replace("'main'", "'isekai'")
        .replace("|main|", "|isekai|")
        .replace(
            " 'isekai', '42',\n                 NULL, 'awaiting_take'",
            &format!(" 'isekai', '42',\n                 '{season}', 'awaiting_take'"),
        );
    let mut transaction = pool.begin().await.expect("开启预停季夹具事务");
    sqlx::query(
        "INSERT INTO public.consignment_season_windows
            (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
             calendar_version, preclose_days)
         VALUES ($1, '2026-isekai-preclose', 'isekai', 'UTC', $2, $3, $4,
                 decode(repeat('ab', 32), 'hex'), 3)",
    )
    .bind(season)
    .bind(now.checked_add_signed(auction_time::Duration::days(-10)).expect("十日前"))
    .bind(now.checked_add_signed(auction_time::Duration::days(1)).expect("一日后"))
    .bind(now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前"))
    .execute(&mut *transaction)
    .await
    .expect("写入预停季");
    sqlx::raw_sql(sqlx::AssertSqlSafe(accepted))
        .execute(&mut *transaction)
        .await
        .expect("写入预停季已接受夹具");
    transaction.commit().await.expect("提交预停季夹具");
    (database, pool.clone())
}

async fn accepted_unknown_season_database() -> (TestDatabase, PgPool) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    let accepted =
        support::accepted_envelope().replace("'main'", "'isekai'").replace("|main|", "|isekai|");
    succeeds(pool, &accepted).await;
    (database, pool.clone())
}

fn listing_id() -> Uuid {
    Uuid::parse_str(LISTING).expect("固定挂单")
}

fn seller_id() -> UserId {
    UserId::from_uuid(Uuid::parse_str(USER_1).expect("固定卖家"))
}

async fn position_state(pool: &PgPool) -> (String, i64, Option<i64>) {
    sqlx::query_as(
        "SELECT state, version, current_generation
         FROM public.consignment_custody_positions WHERE id = $1",
    )
    .bind(Uuid::parse_str(POSITION).expect("固定 position"))
    .fetch_one(pool)
    .await
    .expect("读取 position")
}

async fn assert_take(
    pool: &PgPool,
    change: &CustodyListingChange,
    state: ListingState,
    held_reason: Option<HeldOffMarketReason>,
) {
    assert_eq!(change.listing_id, listing_id());
    assert_eq!(change.state, state);
    assert_eq!(change.held_reason, held_reason);
    assert_eq!(change.position_id, Uuid::parse_str(POSITION).expect("position"));
    assert_eq!(
        change.position_state,
        auction_domain::consignment::deposit::PositionState::InCustody
    );
    let stored: (String, i64, Option<i64>) = position_state(pool).await;
    assert_eq!(stored, ("in_custody".to_owned(), 2, Some(1)));
    let event: (String, Option<Uuid>, Option<i64>) = sqlx::query_as(
        "SELECT event_kind, proof_id, listing_version
         FROM public.consignment_custody_events
         WHERE position_id = $1 AND version = 2",
    )
    .bind(Uuid::parse_str(POSITION).expect("position"))
    .fetch_one(pool)
    .await
    .expect("读取取得事件");
    assert_eq!(event.0, "in_custody");
    assert_eq!(event.1, Some(Uuid::parse_str(PROOF).expect("证明")));
    assert!(event.2.is_some());
}

#[tokio::test]
async fn postgres_take_respects_intent_hold_quota_and_season() {
    {
        let (database, pool) = accepted_database().await;
        let repository = repository(&database).await;
        let change = repository
            .test_apply_take(
                Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
                Uuid::parse_str(DECISION).expect("决定"),
                Uuid::parse_str(PROOF).expect("证明"),
                Uuid::parse_str(ATTEMPT).expect("attempt"),
                listing_id(),
            )
            .await
            .expect("无阻断时取得")
            .pop()
            .expect("一条变更");
        assert_take(&pool, &change, ListingState::Available, None).await;
    }

    {
        let (database, pool) = accepted_database().await;
        let repository = repository(&database).await;
        let withdraw = WithdrawDepositCommand {
            command_id: client_command(40),
            seller_user_id: seller_id(),
            listing_id: listing_id(),
            expected_version: ListingVersion::new(4).expect("版本"),
            request_hash: hash(0x80),
            request_id: None,
        };
        repository.request_withdrawal(withdraw).await.expect("登记撤回意愿");
        let change = repository
            .test_apply_take(
                Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
                Uuid::parse_str(DECISION).expect("决定"),
                Uuid::parse_str(PROOF).expect("证明"),
                Uuid::parse_str(ATTEMPT).expect("attempt"),
                listing_id(),
            )
            .await
            .expect("意愿下取得")
            .pop()
            .expect("一条变更");
        assert_take(&pool, &change, ListingState::WithdrawalPending, None).await;
    }

    {
        let (database, pool) = accepted_database().await;
        let repository = repository(&database).await;
        let hold = SetOperationsHoldCommand {
            admin_actor: "fake-admin".to_owned(),
            command_id: client_command(41),
            listing_id: listing_id(),
            expected_version: ListingVersion::new(4).expect("版本"),
            request_hash: hash(0x81),
            request_id: None,
            reason: ListingDelistReason::new("fixture hold".to_owned()).expect("说明"),
        };
        let held = repository.set_operations_hold(hold).await.expect("设止售");
        assert_eq!(held.state, ListingState::HeldOffMarket);
        let change = repository
            .test_apply_take(
                Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
                Uuid::parse_str(DECISION).expect("决定"),
                Uuid::parse_str(PROOF).expect("证明"),
                Uuid::parse_str(ATTEMPT).expect("attempt"),
                listing_id(),
            )
            .await
            .expect("止售下取得")
            .pop()
            .expect("一条变更");
        assert_take(
            &pool,
            &change,
            ListingState::HeldOffMarket,
            Some(HeldOffMarketReason::OperationsHold),
        )
        .await;
        let release = ReleaseOperationsHoldCommand {
            admin_actor: "fake-admin".to_owned(),
            command_id: client_command(42),
            listing_id: listing_id(),
            expected_version: ListingVersion::new(5).expect("版本"),
            hold_id: held.hold_id.expect("止售 ID"),
            request_hash: hash(0x82),
            request_id: None,
        };
        repository.release_operations_hold(release).await.expect("解除止售");
    }

    {
        let (database, pool) = accepted_database().await;
        let repository = repository_with_max(&database, 1).await;
        succeeds(
            &pool,
            &format!(
                "INSERT INTO public.consignment_listings
                    (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
                     pricing_version, fee_bps, state_version, service_epoch,
                     blacklist_count_at_register, blacklist_baseline_ids, created_at,
                     updated_at, expires_at)
                 VALUES ('{other}', '{USER_1}', 'main', '43', 'consigned', 'resolving',
                         101, 1, 500, 1, 1, 0, ARRAY[]::uuid[],
                         timestamptz '2026-01-01T04:00:00Z',
                         timestamptz '2026-01-01T04:00:00Z',
                         timestamptz '2099-01-01T00:00:00Z');
                 INSERT INTO public.consignment_commands
                    (command_id, schema_version, actor_kind, actor_ref, operation,
                     request_target, request_digest, replay_key_id, listing_id, result_status,
                     result_code, result_body, created_at)
                 VALUES ('{command}', 1, 'public_user', '{USER_1}', 'register', 'new',
                         decode(repeat('17', 32), 'hex'), '{replay}', '{other}', 202,
                         'accepted',
                         '{{\"command_id\":\"{command}\",\"operation\":\"register\",
                            \"listing_id\":\"{other}\",\"listing_version\":1,
                            \"state\":\"resolving\",\"disposition\":\"registered\",
                            \"price\":101}}', timestamptz '2026-01-01T04:00:00Z');
                 INSERT INTO public.consignment_events
                    (listing_id, state_version, command_id, actor_kind, event_kind,
                     from_state, to_state, reason_code, price, occurred_at)
                 VALUES ('{other}', 1, '{command}', 'public_user', 'registered', NULL,
                         'resolving', NULL, 101, timestamptz '2026-01-01T04:00:00Z')",
                other = Uuid::new_v4(),
                command = client_command(99).as_uuid(),
                replay = super::super::support::REPLAY,
            ),
        )
        .await;
        let change = repository
            .test_apply_take(
                Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
                Uuid::parse_str(DECISION).expect("决定"),
                Uuid::parse_str(PROOF).expect("证明"),
                Uuid::parse_str(ATTEMPT).expect("attempt"),
                listing_id(),
            )
            .await
            .expect("额度阻断仍取得")
            .pop()
            .expect("一条变更");
        assert_take(
            &pool,
            &change,
            ListingState::HeldOffMarket,
            Some(HeldOffMarketReason::QuotaBlocked),
        )
        .await;
    }

    {
        let (database, pool) = accepted_season_database().await;
        let repository = repository(&database).await;
        let change = repository
            .test_apply_take(
                Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
                Uuid::parse_str(DECISION).expect("决定"),
                Uuid::parse_str(PROOF).expect("证明"),
                Uuid::parse_str(ATTEMPT).expect("attempt"),
                listing_id(),
            )
            .await
            .expect("季关闭后仍需登记取得")
            .pop()
            .expect("一条变更");
        assert_take(&pool, &change, ListingState::DepositAccepted, None).await;
    }
}

#[tokio::test]
async fn postgres_take_withdrawal_intent_uses_the_new_listing_edge_and_receipt() {
    let (database, pool) = accepted_database().await;
    let repository = repository(&database).await;
    repository
        .request_withdrawal(WithdrawDepositCommand {
            command_id: client_command(200),
            seller_user_id: seller_id(),
            listing_id: listing_id(),
            expected_version: ListingVersion::new(4).expect("版本"),
            request_hash: hash(0xd0),
            request_id: None,
        })
        .await
        .expect("登记撤回意愿");
    let change = repository
        .test_apply_take(
            Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
            Uuid::parse_str(DECISION).expect("决定"),
            Uuid::parse_str(PROOF).expect("证明"),
            Uuid::parse_str(ATTEMPT).expect("attempt"),
            listing_id(),
        )
        .await
        .expect("意愿下取得")
        .pop()
        .expect("一条变更");
    assert_take(&pool, &change, ListingState::WithdrawalPending, None).await;
    let listing: (String, i64) =
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id=$1")
            .bind(listing_id())
            .fetch_one(&pool)
            .await
            .expect("读取挂单");
    assert_eq!(listing, ("withdrawal_pending".to_owned(), 6));
    let event: String = sqlx::query_scalar(
        "SELECT event_kind FROM public.consignment_events
         WHERE listing_id=$1 AND state_version=6",
    )
    .bind(listing_id())
    .fetch_one(&pool)
    .await
    .expect("读取撤回事件");
    assert_eq!(event, "withdrawal_pending");
    let receipt: serde_json::Value = sqlx::query_scalar(
        "SELECT result_body FROM public.consignment_commands WHERE command_id=$1",
    )
    .bind(Uuid::parse_str(PROOF_COMMAND).expect("证明命令"))
    .fetch_one(&pool)
    .await
    .expect("读取证明回执");
    assert_eq!(receipt["listing_changes"][0]["state"], "withdrawal_pending");
    assert_eq!(receipt["custody_changes"][0]["state"], "withdrawal_pending");
    assert!(receipt["custody_changes"][0]["held_reason"].is_null());
}

#[tokio::test]
async fn postgres_take_held_off_market_withdraws_or_keeps_review_reason() {
    {
        let (database, pool) = accepted_database().await;
        let repository = repository(&database).await;
        let held = repository
            .set_operations_hold(SetOperationsHoldCommand {
                admin_actor: "fake-admin".to_owned(),
                command_id: client_command(201),
                listing_id: listing_id(),
                expected_version: ListingVersion::new(4).expect("版本"),
                request_hash: hash(0xd1),
                request_id: None,
                reason: ListingDelistReason::new("fixture hold".to_owned()).expect("说明"),
            })
            .await
            .expect("设止售");
        assert_eq!(held.state, ListingState::HeldOffMarket);
        repository
            .request_withdrawal(WithdrawDepositCommand {
                command_id: client_command(202),
                seller_user_id: seller_id(),
                listing_id: listing_id(),
                expected_version: ListingVersion::new(5).expect("版本"),
                request_hash: hash(0xd2),
                request_id: None,
            })
            .await
            .expect("下架态登记意愿");
        let change = repository
            .test_apply_take(
                Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
                Uuid::parse_str(DECISION).expect("决定"),
                Uuid::parse_str(PROOF).expect("证明"),
                Uuid::parse_str(ATTEMPT).expect("attempt"),
                listing_id(),
            )
            .await
            .expect("下架态取得")
            .pop()
            .expect("一条变更");
        assert_take(&pool, &change, ListingState::WithdrawalPending, None).await;
    }

    {
        let (database, pool) = accepted_database().await;
        let repository = repository(&database).await;
        let held = repository
            .set_operations_hold(SetOperationsHoldCommand {
                admin_actor: "fake-admin".to_owned(),
                command_id: client_command(203),
                listing_id: listing_id(),
                expected_version: ListingVersion::new(4).expect("版本"),
                request_hash: hash(0xd3),
                request_id: None,
                reason: ListingDelistReason::new("fixture hold".to_owned()).expect("说明"),
            })
            .await
            .expect("设止售");
        repository
            .release_operations_hold(ReleaseOperationsHoldCommand {
                admin_actor: "fake-admin".to_owned(),
                command_id: client_command(204),
                listing_id: listing_id(),
                expected_version: ListingVersion::new(5).expect("版本"),
                hold_id: held.hold_id.expect("止售 ID"),
                request_hash: hash(0xd4),
                request_id: None,
            })
            .await
            .expect("解除止售");
        let change = repository
            .test_apply_take(
                Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
                Uuid::parse_str(DECISION).expect("决定"),
                Uuid::parse_str(PROOF).expect("证明"),
                Uuid::parse_str(ATTEMPT).expect("attempt"),
                listing_id(),
            )
            .await
            .expect("复核态取得")
            .pop()
            .expect("一条变更");
        assert_take(&pool, &change, ListingState::HeldOffMarket, Some(HeldOffMarketReason::Review))
            .await;
    }
}

#[tokio::test]
async fn postgres_take_handles_preclose_unknown_and_season_priority() {
    {
        let (database, pool) = accepted_preclose_database().await;
        let repository = repository(&database).await;
        let change = repository
            .test_apply_take(
                Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
                Uuid::parse_str(DECISION).expect("决定"),
                Uuid::parse_str(PROOF).expect("证明"),
                Uuid::parse_str(ATTEMPT).expect("attempt"),
                listing_id(),
            )
            .await
            .expect("预停下取得")
            .pop()
            .expect("一条变更");
        assert_take(&pool, &change, ListingState::DepositAccepted, None).await;
    }

    {
        let (database, pool) = accepted_unknown_season_database().await;
        let repository = repository(&database).await;
        let change = repository
            .test_apply_take(
                Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
                Uuid::parse_str(DECISION).expect("决定"),
                Uuid::parse_str(PROOF).expect("证明"),
                Uuid::parse_str(ATTEMPT).expect("attempt"),
                listing_id(),
            )
            .await
            .expect("未知季取得")
            .pop()
            .expect("一条变更");
        assert_take(&pool, &change, ListingState::HeldOffMarket, Some(HeldOffMarketReason::Review))
            .await;
    }

    {
        let (database, pool) = accepted_season_database().await;
        succeeds(
            &pool,
            &format!(
                "INSERT INTO public.consignment_custody_controls
                    (id, position_id, kind, predecessor_id, command_id, actor_ref, version,
                     occurred_at)
                 VALUES ('{intent}', '{POSITION}', 'withdrawal_intent', NULL, '{DECIDE}',
                         'fixture-seller', 1, transaction_timestamp());
                 INSERT INTO public.consignment_custody_controls
                    (id, position_id, kind, predecessor_id, command_id, actor_ref, version,
                     occurred_at)
                 VALUES ('{hold}', '{POSITION}', 'hold_set', NULL, '{DECIDE}',
                         'fixture-admin', 2, transaction_timestamp())",
                intent = Uuid::new_v4(),
                hold = Uuid::new_v4(),
            ),
        )
        .await;
        let repository = repository(&database).await;
        let change = repository
            .test_apply_take(
                Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
                Uuid::parse_str(DECISION).expect("决定"),
                Uuid::parse_str(PROOF).expect("证明"),
                Uuid::parse_str(ATTEMPT).expect("attempt"),
                listing_id(),
            )
            .await
            .expect("季关闭优先取得")
            .pop()
            .expect("一条变更");
        assert_take(&pool, &change, ListingState::DepositAccepted, None).await;
    }
}

fn rejected_edge_command(command_id: Uuid, operation: &str, listing_id: Uuid) -> String {
    let target_kind = if operation == "apply_mail_proof" { "original_attempt" } else { "listing" };
    let target_id = if operation == "apply_mail_proof" { Uuid::new_v4() } else { listing_id };
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{command_id}', 2, '{actor_kind}', '{actor_ref}', '{operation}',
                 '{target}', decode(repeat('7f', 32), 'hex'), '{target_kind}', '{target_id}',
                 {listing}, 409, 'stale_version',
                 '{{\"schema_version\":2,\"operation\":\"{operation}\",
                    \"disposition\":\"rejected\",\"command_id\":\"{command_id}\",
                    \"code\":\"stale_version\"}}',
                 transaction_timestamp());",
        target = target_id,
        actor_kind = if operation == "apply_mail_proof" { "worker" } else { "admin" },
        actor_ref = if operation == "apply_mail_proof" { "test-worker" } else { "test-admin" },
        listing = if operation == "apply_mail_proof" {
            "NULL".to_owned()
        } else {
            format!("'{listing_id}'")
        },
    )
}

fn held_off_sql(command_id: Uuid, listing_id: Uuid) -> String {
    let hold_id = Uuid::new_v4();
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{command_id}', 2, 'admin', 'test-admin', 'admin_delist',
                 '{listing_id}', decode(repeat('6a', 32), 'hex'), 'listing', '{listing_id}',
                 '{listing_id}', 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                    \"disposition\":\"hold_set\",\"command_id\":\"{command_id}\",
                    \"listing_id\":\"{listing_id}\",\"listing_version\":5,
                    \"state\":\"held_off_market\",\"held_reason\":\"operations_hold\",
                    \"hold_id\":\"{hold_id}\"}}',
                 transaction_timestamp());
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{listing_id}', 5, '{command_id}', 'admin', 'hold_set',
                 'deposit_accepted', 'held_off_market', 'operations_hold', 101,
                 transaction_timestamp());
         UPDATE public.consignment_listings
         SET state='held_off_market', reason_code='operations_hold', state_version=5,
             updated_at=transaction_timestamp()
         WHERE id='{listing_id}' AND state='deposit_accepted' AND state_version=4;"
    )
}

fn withdrawal_edge_sql(
    command_id: Uuid,
    listing_id: Uuid,
    from_state: &str,
    from_version: i64,
) -> String {
    let to_version = from_version + 1;
    format!(
        "UPDATE public.consignment_listings
         SET state='withdrawal_pending', state_version={to_version}, reason_code=NULL,
             updated_at=transaction_timestamp()
         WHERE id='{listing_id}' AND state='{from_state}' AND state_version={from_version};
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{listing_id}', {to_version}, '{command_id}', 'worker', 'withdrawal_pending',
                 '{from_state}', 'withdrawal_pending', NULL, 101, transaction_timestamp());"
    )
}

#[tokio::test]
async fn postgres_take_withdrawal_edge_requires_apply_mail_proof_receipt() {
    {
        let (_database, pool) = accepted_database().await;
        let command = Uuid::new_v4();
        let error = super::super::support::execute(
            &pool,
            &(rejected_edge_command(command, "admin_delist", listing_id())
                + &withdrawal_edge_sql(command, listing_id(), "deposit_accepted", 4)),
        )
        .await
        .expect_err("非 apply_mail_proof 命令不得提交撤回边");
        let error = error.as_database_error().expect("期望 SQLSTATE");
        assert_eq!(error.code().as_deref(), Some("23514"));
        assert_eq!(error.constraint(), Some("consignment_listing_version_event"));
    }

    {
        let (_database, pool) = accepted_database().await;
        let command = Uuid::new_v5(&Uuid::NAMESPACE_URL, b"take-withdrawal-edge-apply-proof");
        let error = super::super::support::execute(
            &pool,
            &(rejected_edge_command(command, "apply_mail_proof", listing_id())
                + &withdrawal_edge_sql(command, listing_id(), "deposit_accepted", 4)),
        )
        .await
        .expect_err("缺少 custody_changes 的证明回执不得提交撤回边");
        let error = error.as_database_error().expect("期望 SQLSTATE");
        assert_eq!(error.code().as_deref(), Some("23514"));
        assert_eq!(error.constraint(), Some("consignment_listing_version_event"));
    }

    {
        let (_database, pool) = accepted_database().await;
        let hold = Uuid::new_v4();
        super::super::support::execute(&pool, &held_off_sql(hold, listing_id()))
            .await
            .expect("写入下架态");
        let command = Uuid::new_v4();
        let error = super::super::support::execute(
            &pool,
            &(rejected_edge_command(command, "admin_delist", listing_id())
                + &withdrawal_edge_sql(command, listing_id(), "held_off_market", 5)),
        )
        .await
        .expect_err("非 apply_mail_proof 命令不得提交下架态撤回边");
        let error = error.as_database_error().expect("期望 SQLSTATE");
        assert_eq!(error.code().as_deref(), Some("23514"));
        assert_eq!(error.constraint(), Some("consignment_listing_version_event"));
    }
}
