//! 季末收口未起写的取得工作（6D6，R5-C3）：未起写关闭且之后零 POST，已起写保留，重放不重复。
//!
//! 只测核心规则：资产外写（未起写的投递被取消、已起写的绝不自动终结）、命令幂等（重放不重复关闭）。

use std::sync::Arc;

use auction_application::consignment::{
    ReservationSecretAad, ReservationSecretCrypto, ReservationSecretCryptoError,
    SealedReservationSecret, SeasonCloseCommand, SeasonCloseTarget,
};
use auction_application::core::{ApplicationError, RequestHash};
use auction_application::delivery::{
    DeliveryAdminError, DeliveryAdminListRequest, DeliveryAdminPort,
};
use auction_application::mail::DispositionWorkStore;
use auction_application::operations::{
    AuditFailureMode, DispositionReadMode, OperationsAlertCollectionOverdueRead,
    OperationsAlertSourceReader,
};
use auction_application::runtime_switches::{RuntimeSwitchRole, RuntimeSwitches};
use auction_domain::consignment::deposit::{TokenText, format_token, parse_token};
use auction_domain::consignment::{CalendarVersion, FeeBps, FrozenSeason, ListingPrice};
use auction_domain::{CommandId, SettlementWorld};
use auction_storage::{DatabaseStore, DepositRepository, DepositSettings};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::{
    ACCEPTED, ATTEMPT, CREDENTIAL, DECISION, DELIVERY, POSITION, PROOF_COMMAND, SESSION, WORK,
    accepted_envelope, succeeds,
};
use crate::support::TestDatabase;

const WINDOW: &str = "8d000000-0000-0000-0000-0000000000d6";
const SEASON: &str = "2026-isekai-take-close";

/// 测试 AEAD：本组用例不签发预约，只满足仓库构造所需的密码边界。
struct TestCrypto;

impl ReservationSecretCrypto for TestCrypto {
    fn seal(
        &self,
        token: &TokenText,
        _aad: ReservationSecretAad,
    ) -> Result<SealedReservationSecret, ReservationSecretCryptoError> {
        SealedReservationSecret::new(
            token.as_str().as_bytes().to_vec(),
            [7_u8; 12],
            "test-key".to_owned(),
            "a".repeat(64),
        )
    }

    fn open(
        &self,
        sealed: &SealedReservationSecret,
        _aad: ReservationSecretAad,
    ) -> Result<TokenText, ReservationSecretCryptoError> {
        let text =
            std::str::from_utf8(sealed.ciphertext()).map_err(|_| ReservationSecretCryptoError)?;
        let token = parse_token(text).map_err(|_| ReservationSecretCryptoError)?;
        Ok(format_token(*token.as_bytes()))
    }
}

struct Window {
    start: auction_time::Timestamp,
    end: auction_time::Timestamp,
    cutoff: auction_time::Timestamp,
}

/// 季末在一小时前的 isekai 季窗，并接受一件尚未取得（awaiting_take）的代管件，取得工作未起写。
async fn seed_awaiting(pool: &PgPool) -> Window {
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let window = Window {
        start: now.checked_add_signed(auction_time::Duration::days(-10)).expect("十日前"),
        end: now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前"),
        cutoff: now.checked_add_signed(auction_time::Duration::days(-2)).expect("两日前"),
    };
    sqlx::query(
        "INSERT INTO public.consignment_season_windows
            (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
             calendar_version, preclose_days)
         VALUES ($1::uuid, $2, 'isekai', 'UTC', $3, $4, $5, $6, 3)",
    )
    .bind(WINDOW)
    .bind(SEASON)
    .bind(window.start)
    .bind(window.end)
    .bind(window.cutoff)
    .bind([0x79_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入季窗");
    let accepted =
        accepted_envelope().replace("'main'", "'isekai'").replace("|main|", "|isekai|").replace(
            " 'isekai', '42',\n                 NULL, 'awaiting_take'",
            &format!(" 'isekai', '42',\n                 '{WINDOW}', 'awaiting_take'"),
        );
    assert!(accepted.contains(WINDOW), "isekai 夹具必须绑定季窗");
    succeeds(pool, &accepted).await;
    window
}

/// 取得工作已起写（外写许可已发出、结果未证明）：命令、尝试与起写事实。
async fn seed_write_started(pool: &PgPool) {
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{PROOF_COMMAND}', 2, 'worker', 'fake-worker', 'apply_mail_proof',
                     '{ATTEMPT}', decode(repeat('a5', 32), 'hex'), 'original_attempt',
                     '{ATTEMPT}', NULL, 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"apply_mail_proof\",
                        \"disposition\":\"take_proven\",\"command_id\":\"{PROOF_COMMAND}\",
                        \"decision_id\":\"{DECISION}\",\"proof_id\":\"{ATTEMPT}\"}}',
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
        ),
    )
    .await;
}

/// isekai 的邮件就绪行：让取得工作可被领取（夹具信封已带 isekai 邮件会话）。
async fn seed_readiness(pool: &PgPool) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('isekai', '{SESSION}', 1, decode(repeat('36', 32), 'hex'), 'enforced',
                 ARRAY['FakeSellerOne'], ARRAY[]::text[], true, true, true,
                 clock_timestamp(), clock_timestamp() + interval '1 day', 1);"
    )))
    .execute(pool)
    .await
    .expect("写入 isekai 就绪");
}

/// 给取得投递补上冻结清单（夹具载荷没有），使领取路径可解码。
async fn seed_manifest(pool: &PgPool) {
    let digest: Vec<u8> = vec![0x68; 32];
    let manifest = serde_json::json!({
        "world": "isekai", "attachments": [], "body_digest": digest,
        "external_mail_id": "555", "sender": "FakeSellerOne", "recipient": "FakeBot",
        "subject": "fake", "credits": 0, "cod": 0,
        "received_minute_raw": "2026-01-01T05:00", "received_at": "2026-01-01T03:00:00Z",
    });
    sqlx::query(
        "UPDATE public.deliveries
         SET payload = payload || jsonb_build_object('expected_manifest', $2::jsonb)
         WHERE id = $1::uuid",
    )
    .bind(DELIVERY)
    .bind(manifest)
    .execute(pool)
    .await
    .expect("补冻结清单");
}

async fn repository(database: &TestDatabase) -> DepositRepository {
    DepositRepository::new(
        database.store().await.expect("运行账号"),
        Arc::new(TestCrypto),
        DepositSettings::new(
            FeeBps::new(500).expect("测试费率"),
            ListingPrice::new(ListingPrice::MIN).expect("测试下限"),
            ListingPrice::new(ListingPrice::MAX).expect("测试上限"),
            24,
            100,
        ),
    )
}

fn close_command(seed: u128, window: &Window) -> SeasonCloseCommand {
    SeasonCloseCommand {
        command_id: CommandId::from_uuid(Uuid::from_u128(
            0x0000_0000_0000_5000_8000_0000_0000_0000 + seed,
        )),
        season: FrozenSeason {
            world: SettlementWorld::Isekai,
            season_id: SEASON.to_owned(),
            start_utc: window.start,
            end_utc: window.end,
            admission_cutoff_utc: window.cutoff,
            calendar_version: CalendarVersion::from_bytes([0x79; 32]),
            preclose_days: 3,
        },
        target: SeasonCloseTarget::World(SettlementWorld::Isekai),
        request_hash: RequestHash::new([0x7a; 32]),
    }
}

async fn position_state(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT state FROM public.consignment_custody_positions WHERE id=$1::uuid")
        .bind(POSITION)
        .fetch_one(pool)
        .await
        .expect("读取 position")
}

/// 取得投递的 (状态, 围栏)。
async fn delivery(pool: &PgPool) -> (String, i64) {
    sqlx::query_as("SELECT status, fencing_version FROM public.deliveries WHERE id=$1::uuid")
        .bind(DELIVERY)
        .fetch_one(pool)
        .await
        .expect("读取投递")
}

async fn count(pool: &PgPool, sql: &'static str, id: &str) -> i64 {
    sqlx::query_scalar(sql).bind(id).fetch_one(pool).await.expect("计数")
}

const CLOSURES: &str =
    "SELECT count(*) FROM public.consignment_work_closures WHERE work_id = $1::uuid";
const STARTS: &str =
    "SELECT count(*) FROM public.mail_disposition_write_starts WHERE work_id = $1::uuid";
const SEASON_END_EVENTS: &str = "SELECT count(*) FROM public.consignment_custody_events
     WHERE event_kind = 'season_ended' AND position_id = $1::uuid";

#[tokio::test]
async fn postgres_consignment_season_take_close_cancels_unstarted_work_and_replays_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_awaiting(pool).await;
    seed_readiness(pool).await;
    seed_manifest(pool).await;
    let repository = repository(&database).await;
    assert_eq!(position_state(pool).await, "awaiting_take");
    // 季末前 Worker 已领取（持租约、未起写）：围栏 1。
    let now = auction_time::Timestamp::now();
    let claim = repository
        .claim_due("season-take-worker".to_owned(), now, now + auction_time::Duration::minutes(5))
        .await
        .expect("领取")
        .expect("季末前可领取");
    assert_eq!(delivery(pool).await, ("claimed".to_owned(), 1));

    repository.test_close_season_at(close_command(1, &window), window.end).await.expect("关季");
    // 正例：关闭记录落地，投递被取消（围栏前进、不再可领取），没有任何起写，实物占用已释放。
    assert_eq!(position_state(pool).await, "season_ended");
    assert_eq!(count(pool, CLOSURES, WORK).await, 1);
    assert_eq!(delivery(pool).await, ("failed".to_owned(), 2));
    assert_eq!(count(pool, STARTS, WORK).await, 0);
    // 之后零 POST：旧领取的租约已失效（起写要求 claimed 且围栏相等），工作不再可被领取。
    repository.check_current(&claim).await.expect_err("旧领取不再有效");
    let later = auction_time::Timestamp::now();
    let none = repository
        .claim_due(
            "season-take-worker".to_owned(),
            later,
            later + auction_time::Duration::minutes(5),
        )
        .await
        .expect("再次领取");
    assert!(none.is_none(), "关闭后的工作不得再被领取");
    assert_eq!(count(pool, SEASON_END_EVENTS, POSITION).await, 1);

    // 重放：同命令原样返回；另一条命令再关同一季也不重复关闭、不再推进围栏。
    repository
        .test_close_season_at(close_command(1, &window), window.end)
        .await
        .expect("同命令重放");
    repository.test_close_season_at(close_command(2, &window), window.end).await.expect("再次关季");
    assert_eq!(count(pool, CLOSURES, WORK).await, 1);
    assert_eq!(count(pool, SEASON_END_EVENTS, POSITION).await, 1);
    assert_eq!(delivery(pool).await, ("failed".to_owned(), 2));
}

#[tokio::test]
async fn postgres_consignment_season_take_close_keeps_started_work_for_reconcile() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_awaiting(pool).await;
    seed_write_started(pool).await;
    let repository = repository(&database).await;

    repository.test_close_season_at(close_command(3, &window), window.end).await.expect("关季");
    // 反例：已起写、结果未证明的取得工作不终结、不关闭、不取消投递，留给只读核对。
    assert_eq!(position_state(pool).await, "awaiting_take");
    assert_eq!(count(pool, CLOSURES, WORK).await, 0);
    assert_eq!(count(pool, STARTS, WORK).await, 1);
    assert_eq!(delivery(pool).await, ("prepared".to_owned(), 0));
}

#[tokio::test]
async fn postgres_consignment_season_take_close_rolls_back_everything_when_delivery_cancel_fails() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_awaiting(pool).await;
    let repository = repository(&database).await;
    let command = close_command(4, &window);
    let claim_before: (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT reservation_id, position_id FROM public.consignment_equipment_claims
         WHERE world_id='isekai' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");

    // 触发器吞掉投递取消使 rows_affected 为 0：取得关闭必须整封失败，不留半截。
    sqlx::raw_sql(
        "CREATE FUNCTION public.p6d6_fault() RETURNS trigger LANGUAGE plpgsql AS $f$
         BEGIN IF NEW.status = 'failed' THEN RETURN NULL; END IF; RETURN NEW; END $f$;
         CREATE TRIGGER a_p6d6_fault BEFORE UPDATE ON public.deliveries
         FOR EACH ROW EXECUTE FUNCTION public.p6d6_fault();",
    )
    .execute(pool)
    .await
    .expect("安装故障触发器");
    repository
        .test_close_season_at(command.clone(), window.end)
        .await
        .expect_err("投递取消失败必须整封失败");
    assert_eq!(position_state(pool).await, "awaiting_take");
    assert_eq!(count(pool, CLOSURES, WORK).await, 0);
    assert_eq!(count(pool, SEASON_END_EVENTS, POSITION).await, 0);
    assert_eq!(delivery(pool).await, ("prepared".to_owned(), 0));
    let claim_after: (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT reservation_id, position_id FROM public.consignment_equipment_claims
         WHERE world_id='isekai' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(claim_after, claim_before);

    sqlx::raw_sql(
        "DROP TRIGGER a_p6d6_fault ON public.deliveries; DROP FUNCTION public.p6d6_fault();",
    )
    .execute(pool)
    .await
    .expect("移除故障触发器");
    repository.test_close_season_at(command, window.end).await.expect("故障移除后重试");
    assert_eq!(position_state(pool).await, "season_ended");
}

/// 告警快照（分类口径：统一处置事实 `resolves_alert`）里投递来源的未处置失败数。
async fn classified_delivery_failed(store: &DatabaseStore) -> i64 {
    store
        .clone()
        .with_runtime_switches(
            RuntimeSwitches::new(RuntimeSwitchRole::Worker, AuditFailureMode::Shadow, true)
                .with_disposition_read_mode_baseline(DispositionReadMode::Classified),
        )
        .snapshot(auction_time::Timestamp::now(), OperationsAlertCollectionOverdueRead::Skip)
        .await
        .expect("读取告警快照")
        .delivery
        .expect("投递水位")
        .failed
}

/// 一条与寄售无关、没有任何处置事实的普通失败投递。
async fn insert_plain_failed_delivery(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             created_at, updated_at, failure_code, result_metadata)
         VALUES ($1, $2, decode(repeat('33', 32), 'hex'), 'mail_intake', $3, 'player-plain',
                 '{\"schema_version\":1,\"operation\":\"send\",\"template\":\"register_key\",\"credits\":0,\"cod\":0,\"attachment_ids\":[]}'::jsonb,
                 'failed', 1, 1, clock_timestamp(), clock_timestamp(), clock_timestamp(),
                 clock_timestamp(), clock_timestamp(), 'verified_not_sent', '{}'::jsonb)",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(format!("p6d6-plain:{id}"))
    .execute(pool)
    .await
    .expect("写入普通失败投递");
    id
}

/// 直接写一条 `season_closed` 处置；返回数据库拒绝时违反的约束名。
async fn insert_season_closed(pool: &PgPool, delivery_id: Uuid, fence: i64) -> Option<String> {
    sqlx::query(
        "INSERT INTO public.delivery_dispositions
            (delivery_id, fencing_version, kind, actor_source, actor_id, reason_code,
             resolves_alert)
         VALUES ($1, $2, 'season_closed', 'worker', 'consignment-worker', 'season_ended', true)",
    )
    .bind(delivery_id)
    .bind(fence)
    .execute(pool)
    .await
    .err()
    .map(|error| {
        error
            .as_database_error()
            .and_then(|database| database.constraint().map(str::to_owned))
            .unwrap_or_else(|| format!("非约束错误: {error}"))
    })
}

#[tokio::test]
async fn postgres_consignment_season_take_close_disposition_resolves_only_closed_take_delivery() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_awaiting(pool).await;
    let plain = insert_plain_failed_delivery(pool).await;
    let store = database.store().await.expect("运行账号");
    let take = Uuid::parse_str(DELIVERY).expect("取得投递");
    // 反例：普通失败投递没有处置，照旧计入；取得投递尚未关闭时写 season_closed 被拒。
    assert_eq!(classified_delivery_failed(&store).await, 1);
    assert_eq!(
        insert_season_closed(pool, take, 1).await.as_deref(),
        Some("c7_season_closed_disposition")
    );

    let repository = repository(&database).await;
    repository.test_close_season_at(close_command(5, &window), window.end).await.expect("关季");
    // 正例：取消后的取得投递在当前 fence 上带 season_closed 处置（自证、解除告警）。
    assert_eq!(delivery(pool).await, ("failed".to_owned(), 1));
    let disposition: (String, i64, bool, String, Option<String>) = sqlx::query_as(
        "SELECT kind, fencing_version, resolves_alert, actor_source, reason_code
         FROM public.delivery_dispositions WHERE delivery_id = $1",
    )
    .bind(take)
    .fetch_one(pool)
    .await
    .expect("读取季末关闭处置");
    assert_eq!(
        disposition,
        ("season_closed".to_owned(), 1, true, "worker".to_owned(), Some("season_ended".to_owned()))
    );
    // 告警快照不再把关闭的取得投递算作未处置失败，普通失败仍计入。
    assert_eq!(classified_delivery_failed(&store).await, 1);

    // 反例：别的投递（普通失败）写 season_closed 被数据库守卫拒绝；取得投递的非当前 fence 同样被拒。
    assert_eq!(
        insert_season_closed(pool, plain, 1).await.as_deref(),
        Some("c7_season_closed_disposition")
    );
    assert_eq!(
        insert_season_closed(pool, take, 2).await.as_deref(),
        Some("c7_season_closed_disposition")
    );
}

/// Admin 列表（缺省或扩展投影）里取得投递的状态；读取失败原样返回。
async fn admin_list_status(
    store: &DatabaseStore,
    include_extended: bool,
) -> Result<Option<String>, ApplicationError> {
    let page = store
        .list(DeliveryAdminListRequest {
            include_settlement_return_actions: include_extended,
            limit: 100,
            offset: 0,
            ..DeliveryAdminListRequest::default()
        })
        .await?;
    let take = Uuid::parse_str(DELIVERY).expect("取得投递");
    Ok(page.entries.iter().find(|row| row.id == take).map(|row| row.status.clone()))
}

fn corrupt(result: Result<impl std::fmt::Debug, ApplicationError>) -> bool {
    matches!(result, Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::StoredResultCorrupt)))
}

#[tokio::test]
async fn postgres_consignment_season_take_close_delivery_stays_readable_by_admin() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let window = seed_awaiting(pool).await;
    let repository = repository(&database).await;
    repository.test_close_season_at(close_command(6, &window), window.end).await.expect("关季");
    let store = database.store().await.expect("运行账号");
    let take = Uuid::parse_str(DELIVERY).expect("取得投递");

    // 正例：季末取消的 failed/season_ended 取得投递不让 Admin 列表或详情整体失败。
    for include_extended in [false, true] {
        assert_eq!(
            admin_list_status(&store, include_extended).await.expect("Admin 列表"),
            Some("failed".to_owned()),
            "include_extended={include_extended}"
        );
    }
    let detail = store.detail(take).await.expect("Admin 详情");
    assert_eq!(detail.delivery.status, "failed");
    assert_eq!(detail.delivery.failure_code.as_deref(), Some("season_ended"));
    assert!(detail.dispositions.iter().any(|item| item.kind == "season_closed"));

    // 反例：同一来源的其他 failed 形状仍按损坏失败关闭。
    for (column, value) in [("failure_code", "'send_rejected'"), ("sending_at", "completed_at")] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "UPDATE public.deliveries SET {column} = {value} WHERE id = '{DELIVERY}'"
        )))
        .execute(pool)
        .await
        .expect("注入非法形状");
        assert!(corrupt(admin_list_status(&store, true).await), "{column}");
        assert!(corrupt(store.detail(take).await), "{column}");
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "UPDATE public.deliveries SET failure_code = 'season_ended', sending_at = NULL
             WHERE id = '{DELIVERY}'"
        )))
        .execute(pool)
        .await
        .expect("恢复形状");
    }
}
