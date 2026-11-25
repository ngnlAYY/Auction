//! 5E 季末终态：关季事务把季窗内的代管件写成 season_ended 并释放实物占用。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, ReservationStore, SeasonCloseCommand,
    SeasonCloseTarget, SeasonStore,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::{CalendarVersion, FrozenSeason};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::support::{self, LISTING, POSITION};
use super::helpers::{
    client_command, hash, issue_command, repository, seed_listing, worker_command,
};
use crate::support::TestDatabase;

const SEASON: &str = "2026-isekai-end";

struct Window {
    id: Uuid,
    start: auction_time::Timestamp,
    end: auction_time::Timestamp,
    cutoff: auction_time::Timestamp,
    calendar: [u8; 32],
}

fn position_id() -> Uuid {
    Uuid::parse_str(POSITION).expect("固定 position")
}

/// 季末在一小时前的旧季窗；挂单登记于 2026-01-01，早于季末。
async fn seed_ended_window(pool: &PgPool) -> Window {
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let start = now.checked_add_signed(auction_time::Duration::days(-10)).expect("十日前");
    let end = now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前");
    let cutoff = now.checked_add_signed(auction_time::Duration::days(-2)).expect("两日前");
    let window = Window { id: Uuid::new_v4(), start, end, cutoff, calendar: [0x79; 32] };
    insert_window(pool, &window, SEASON).await;
    window
}

async fn insert_window(pool: &PgPool, window: &Window, season_id: &str) {
    sqlx::query(
        "INSERT INTO public.consignment_season_windows
            (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
             calendar_version, preclose_days)
         VALUES ($1, $2, 'isekai', 'UTC', $3, $4, $5, $6, 3)",
    )
    .bind(window.id)
    .bind(season_id)
    .bind(window.start)
    .bind(window.end)
    .bind(window.cutoff)
    .bind(window.calendar.as_slice())
    .execute(pool)
    .await
    .expect("写入季窗");
}

/// 旧季里已被接受的 isekai 位置；`in_custody` 为真时再叠加取得证明与代管事件。
async fn seed_position(pool: &PgPool, window: &Window, in_custody: bool) {
    let rewrite = |sql: String| {
        sql.replace("'main'", "'isekai'").replace("|main|", "|isekai|").replace(
            " 'isekai', '42',\n                 NULL, 'awaiting_take'",
            &format!(" 'isekai', '42',\n                 '{}', 'awaiting_take'", window.id),
        )
    };
    // 整封接受要求提交时 position 仍是 awaiting_take，取得推进只能放到后一个事务。
    support::succeeds(pool, &rewrite(support::accepted_envelope())).await;
    if in_custody {
        support::succeeds(pool, &rewrite(support::attempt_start_proof_and_custody())).await;
    }
}

fn close_command(seed: u128, window: &Window) -> SeasonCloseCommand {
    SeasonCloseCommand {
        command_id: worker_command(seed),
        season: FrozenSeason {
            world: SettlementWorld::Isekai,
            season_id: SEASON.to_owned(),
            start_utc: window.start,
            end_utc: window.end,
            admission_cutoff_utc: window.cutoff,
            calendar_version: CalendarVersion::from_bytes(window.calendar),
            preclose_days: 3,
        },
        target: SeasonCloseTarget::World(SettlementWorld::Isekai),
        request_hash: hash(0x7a),
    }
}

async fn position_state(pool: &PgPool) -> (String, i64) {
    sqlx::query_as("SELECT state, version FROM public.consignment_custody_positions WHERE id=$1")
        .bind(position_id())
        .fetch_one(pool)
        .await
        .expect("读取 position")
}

async fn claim(pool: &PgPool) -> (Option<Uuid>, Option<Uuid>, i64) {
    sqlx::query_as(
        "SELECT reservation_id, position_id, revision FROM public.consignment_equipment_claims
         WHERE world_id='isekai' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用")
}

async fn season_end_events(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_custody_events
         WHERE position_id=$1 AND event_kind='season_ended'",
    )
    .bind(position_id())
    .fetch_one(pool)
    .await
    .expect("读取终态事件")
}

#[tokio::test]
async fn postgres_season_end_terminates_only_at_or_after_the_frozen_end() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_ended_window(pool).await;
    seed_position(pool, &window, true).await;
    let repository = repository(&database).await;
    let before =
        window.end.checked_add_signed(auction_time::Duration::microseconds(-1)).expect("前");
    let error = repository
        .test_close_season_at(close_command(1, &window), before)
        .await
        .expect_err("季末前一刻不得终结");
    assert!(matches!(error, ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed)));
    assert_eq!(position_state(pool).await, ("in_custody".to_owned(), 2));
    let held = claim(pool).await;
    assert_eq!((held.0, held.1), (None, Some(position_id())));
    assert_eq!(season_end_events(pool).await, 0);

    repository
        .test_close_season_at(close_command(2, &window), window.end)
        .await
        .expect("等号即终结");
    assert_eq!(position_state(pool).await, ("season_ended".to_owned(), 3));
    let released = claim(pool).await;
    assert_eq!(released, (None, None, held.2 + 1));
    let event: (Option<Uuid>, String) = sqlx::query_as(
        "SELECT proof_id, from_state FROM public.consignment_custody_events
         WHERE position_id=$1 AND version=3 AND event_kind='season_ended'",
    )
    .bind(position_id())
    .fetch_one(pool)
    .await
    .expect("终态事件");
    assert_eq!(event, (None, "in_custody".to_owned()));
}

#[tokio::test]
async fn postgres_season_end_terminates_reconcile_and_unstarted_awaiting_take() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_ended_window(pool).await;
    seed_position(pool, &window, false).await;
    let store = repository(&database).await;
    store.test_close_season_at(close_command(3, &window), window.end).await.expect("关闭旧季");
    // 未起写的 awaiting_take 随季末收口（6D6，R5-C3）：终结、释放占用、写一条季末事件；
    // 已起写的保留见 returns/season_take.rs。
    assert_eq!(position_state(pool).await, ("season_ended".to_owned(), 2));
    assert_eq!(claim(pool).await.1, None);
    assert_eq!(season_end_events(pool).await, 1);

    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_ended_window(pool).await;
    seed_position(pool, &window, true).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "UPDATE public.consignment_custody_positions
         SET state='reconcile', version=3, updated_at='2026-01-02T00:00:00Z' WHERE id='{POSITION}';
         INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 3, '{}', 'reconcile', 'in_custody', 'reconcile', '{}',
                 '{LISTING}', 4, clock_timestamp());",
        support::PROOF_COMMAND,
        support::PROOF,
    )))
    .execute(pool)
    .await
    .expect("进入核对");
    let repository = repository(&database).await;
    repository.test_close_season_at(close_command(4, &window), window.end).await.expect("关闭旧季");
    assert_eq!(position_state(pool).await, ("season_ended".to_owned(), 4));
}

#[tokio::test]
async fn postgres_season_end_releases_the_claim_for_a_new_season_and_clears_work() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_ended_window(pool).await;
    seed_position(pool, &window, true).await;
    // 夹具里的取得投递视为已收口，剩下的存量只来自 position 与挂单。
    sqlx::query("UPDATE public.deliveries SET status='failed', failure_code='fixture', completed_at='2026-01-01T04:00:00Z',
             updated_at='2026-01-01T04:00:00Z' WHERE id=$1")
        .bind(Uuid::parse_str(support::DELIVERY).expect("投递"))
        .execute(pool)
        .await
        .expect("收口投递");
    let runtime = database.store().await.expect("运行账号");
    assert!(runtime.has_consignment_work().await.expect("终态前有存量"));
    let repository = repository(&database).await;
    repository.test_close_season_at(close_command(5, &window), window.end).await.expect("关闭旧季");
    assert!(!runtime.has_consignment_work().await.expect("终态后无存量"));

    // 同 EID 在新季可登记：占用已释放，新挂单能取得预约。
    let next = Window {
        id: Uuid::new_v4(),
        start: window.end,
        end: window.end.checked_add_signed(auction_time::Duration::days(30)).expect("三十日"),
        cutoff: window.end.checked_add_signed(auction_time::Duration::days(20)).expect("二十日"),
        calendar: [0x7b; 32],
    };
    insert_window(pool, &next, "2026-isekai-next").await;
    sqlx::query("UPDATE public.consignment_replay_keys SET write_current = false")
        .execute(pool)
        .await
        .expect("让出当前重放钥");
    let listing = Uuid::new_v4();
    seed_listing(pool, listing, SettlementWorld::Isekai).await;
    // 夹具信封已带 isekai 的邮件会话；只补就绪行与闸门。
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('isekai', '{}', 1, decode(repeat('36', 32), 'hex'), 'enforced',
                 ARRAY['FakeSellerOne'], ARRAY[]::text[], true, true, true,
                 clock_timestamp(), clock_timestamp() + interval '1 day', 1);
         UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1;",
        support::SESSION,
    )))
    .execute(pool)
    .await
    .expect("写入 isekai 就绪");
    repository
        .issue_reservation(issue_command(client_command(80), listing, 2, 0x7c))
        .await
        .expect("新季同号可登记");
    let held = claim(pool).await;
    assert!(held.0.is_some() && held.1.is_none(), "{held:?}");
}

#[tokio::test]
async fn postgres_season_end_replay_does_not_rewrite() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_ended_window(pool).await;
    seed_position(pool, &window, true).await;
    let repository = repository(&database).await;
    let command = close_command(6, &window);
    let first = repository.close_season(command.clone()).await.expect("首次关季");
    let after_first = (position_state(pool).await, claim(pool).await);
    let replay = repository.close_season(command).await.expect("重放");
    assert_eq!(replay, first);
    assert_eq!((position_state(pool).await, claim(pool).await), after_first);
    assert_eq!(season_end_events(pool).await, 1);
}

/// 在 position 或占用的更新上装一个测试专用触发器，制造关季事务中途的失败。
async fn inject(pool: &PgPool, table: &str, body: &str) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE FUNCTION public.p5e_fault() RETURNS trigger LANGUAGE plpgsql AS $f$
         BEGIN {body} END $f$;
         CREATE TRIGGER a_p5e_fault BEFORE UPDATE ON public.{table}
         FOR EACH ROW EXECUTE FUNCTION public.p5e_fault();"
    )))
    .execute(pool)
    .await
    .expect("安装故障触发器");
}

async fn remove_fault(pool: &PgPool, table: &str) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP TRIGGER a_p5e_fault ON public.{table}; DROP FUNCTION public.p5e_fault();"
    )))
    .execute(pool)
    .await
    .expect("移除故障触发器");
}

async fn listing_state(pool: &PgPool) -> (String, i64) {
    sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id=$1")
        .bind(Uuid::parse_str(LISTING).expect("固定挂单"))
        .fetch_one(pool)
        .await
        .expect("读取挂单")
}

async fn receipts(pool: &PgPool, command: &SeasonCloseCommand) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.consignment_commands WHERE command_id=$1")
        .bind(command.command_id.as_uuid())
        .fetch_one(pool)
        .await
        .expect("读取命令账本")
}

/// 故障注入后整封回滚：position、终态事件、占用释放、挂单关闭与回执都不留痕；
/// 移除故障后同一命令可重试成功。
async fn assert_close_is_all_or_nothing(table: &str, body: &str, seed: u128) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_ended_window(pool).await;
    seed_position(pool, &window, true).await;
    let repository = repository(&database).await;
    let listing_before = listing_state(pool).await;
    let claim_before = claim(pool).await;
    let command = close_command(seed, &window);

    inject(pool, table, body).await;
    repository
        .test_close_season_at(command.clone(), window.end)
        .await
        .expect_err("事务中途失败必须整封失败");
    assert_eq!(position_state(pool).await, ("in_custody".to_owned(), 2));
    assert_eq!(season_end_events(pool).await, 0);
    assert_eq!(claim(pool).await, claim_before);
    assert_eq!(listing_state(pool).await, listing_before);
    assert_eq!(receipts(pool, &command).await, 0);

    remove_fault(pool, table).await;
    repository.test_close_season_at(command.clone(), window.end).await.expect("故障移除后重试");
    assert_eq!(position_state(pool).await, ("season_ended".to_owned(), 3));
    assert_eq!(season_end_events(pool).await, 1);
    assert_eq!(receipts(pool, &command).await, 1);
}

#[tokio::test]
async fn postgres_season_end_rolls_back_everything_when_claim_release_fails() {
    assert_close_is_all_or_nothing(
        "consignment_equipment_claims",
        "RAISE EXCEPTION 'p5e 注入：占用释放失败'; RETURN NEW;",
        7,
    )
    .await;
}

#[tokio::test]
async fn postgres_season_end_fails_closed_when_the_position_update_matches_no_row() {
    // 触发器吞掉 position 更新使 rows_affected 为 0，关季必须失败而不是继续写事件。
    assert_close_is_all_or_nothing(
        "consignment_custody_positions",
        "IF NEW.state = 'season_ended' THEN RETURN NULL; END IF; RETURN NEW;",
        8,
    )
    .await;
}

async fn targeted_close_ends_the_position(target: fn() -> SeasonCloseTarget, seed: u128) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_ended_window(pool).await;
    seed_position(pool, &window, true).await;
    let repository = repository(&database).await;
    let mut command = close_command(seed, &window);
    command.target = target();
    repository.test_close_season_at(command, window.end).await.expect("定向关季");
    assert_eq!(position_state(pool).await, ("season_ended".to_owned(), 3));
    assert_eq!(season_end_events(pool).await, 1);
    assert_eq!(claim(pool).await.1, None);
    assert_eq!(listing_state(pool).await.0, "closed");
}

#[tokio::test]
async fn postgres_season_end_listing_target_ends_its_position() {
    targeted_close_ends_the_position(
        || SeasonCloseTarget::Listing(Uuid::parse_str(LISTING).expect("固定挂单")),
        9,
    )
    .await;
}

#[tokio::test]
async fn postgres_season_end_position_target_ends_its_position() {
    targeted_close_ends_the_position(|| SeasonCloseTarget::Position(position_id()), 10).await;
}
