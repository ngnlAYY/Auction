//! 季任务端口：季窗只冻结一次，季前通知、Send 工作与投递的派生身份保持幂等。

use auction_application::consignment::{
    SeasonNoticeKind, SeasonTaskStore, season_notice_delivery_command, season_notice_delivery_id,
    season_notice_work_id,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::{CalendarVersion, FrozenSeason};
use auction_time::{SecondsFormat, Timestamp};
use sqlx::AssertSqlSafe;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::support::{POSITION, USER_1};
use super::helpers::{command_service, repository};
use crate::support::TestDatabase;

struct SeasonFixture {
    id: Uuid,
    season: FrozenSeason,
}

async fn seed_season(pool: &PgPool, season_id: &str, preclose: bool) -> SeasonFixture {
    let now: Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let start = now.checked_add_signed(auction_time::Duration::days(-10)).expect("十日前");
    let end = now.checked_add_signed(auction_time::Duration::days(10)).expect("十日后");
    let cutoff = if preclose {
        now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前")
    } else {
        now.checked_add_signed(auction_time::Duration::hours(1)).expect("一小时后")
    };
    let calendar = CalendarVersion::from_bytes([0x79; 32]);
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.consignment_season_windows
            (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
             calendar_version, preclose_days)
         VALUES ($1, $2, 'isekai', 'UTC', $3, $4, $5, $6, 3)",
    )
    .bind(id)
    .bind(season_id)
    .bind(start)
    .bind(end)
    .bind(cutoff)
    .bind(calendar.as_bytes().as_slice())
    .execute(pool)
    .await
    .expect("写入预停季窗");
    SeasonFixture {
        id,
        season: FrozenSeason {
            world: SettlementWorld::Isekai,
            season_id: season_id.to_owned(),
            start_utc: start,
            end_utc: end,
            admission_cutoff_utc: cutoff,
            calendar_version: calendar,
            preclose_days: 3,
        },
    }
}

async fn seed_accepted_in_season(pool: &PgPool, season: &SeasonFixture) {
    let accepted = super::super::support::accepted_envelope()
        .replace("'main'", "'isekai'")
        .replace("|main|", "|isekai|")
        .replace(
            " 'isekai', '42',\n                 NULL, 'awaiting_take'",
            &format!(" 'isekai', '42',\n                 '{}', 'awaiting_take'", season.id),
        );
    let mut transaction = pool.begin().await.expect("开启季内接受事务");
    sqlx::raw_sql(AssertSqlSafe(accepted))
        .execute(&mut *transaction)
        .await
        .expect("写入季内已接受夹具");
    transaction.commit().await.expect("提交季内已接受夹具");
}

fn seller() -> auction_domain::UserId {
    auction_domain::UserId::from_uuid(Uuid::parse_str(USER_1).expect("固定卖家"))
}

#[tokio::test]
async fn postgres_consignment_season_notice_send_is_idempotent() {
    // 反例：季窗尚未到预停点时，同季窗内的 position 不产生通知。
    {
        let open_database = TestDatabase::create().await.expect("克隆模板库");
        let open_maintenance = open_database.maintenance_store().await.expect("维护连接");
        let open_pool = open_maintenance.postgres_pool();
        let open = seed_season(open_pool, "2026-02-01", false).await;
        seed_accepted_in_season(open_pool, &open).await;
        let open_repository = repository(&open_database).await;
        assert!(
            open_repository
                .season_notice_targets(&open.season)
                .await
                .expect("读取未到预停的季窗")
                .is_empty()
        );
        assert!(
            open_repository
                .season_notice_deliveries(&open.season)
                .await
                .expect("读取未到预停的投递")
                .is_empty()
        );
        let open_notices: i64 =
            sqlx::query_scalar("SELECT count(*) FROM public.consignment_season_notices")
                .fetch_one(open_pool)
                .await
                .expect("读取未到预停的通知数");
        assert_eq!(open_notices, 0);
    }

    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let fixture = seed_season(pool, "2026-05-01", true).await;
    seed_accepted_in_season(pool, &fixture).await;
    let repository = repository(&database).await;
    let service = command_service(&repository);
    let position_id = Uuid::parse_str(POSITION).expect("固定 position");

    let targets =
        repository.season_notice_targets(&fixture.season).await.expect("读取季前通知目标");
    assert_eq!(targets.len(), 1, "同一 position 只应排队一次");
    assert_eq!(targets[0].position_id, position_id);
    assert_eq!(targets[0].seller_user_id, seller());
    service
        .queue_season_notice(
            fixture.season.clone(),
            position_id,
            seller(),
            SeasonNoticeKind::PrecloseWithdrawal,
        )
        .await
        .expect("排队季前通知");

    // 第二轮：通知与工作都已存在，扫描不再产生新行。
    assert!(
        repository
            .season_notice_targets(&fixture.season)
            .await
            .expect("重扫季前通知目标")
            .is_empty()
    );
    let pending = repository.season_notice_deliveries(&fixture.season).await.expect("读取待建投递");
    assert_eq!(pending.len(), 1, "同一通知只应建立一份投递");
    let command = season_notice_delivery_command(
        &fixture.season,
        pending[0].notice_id,
        &pending[0].seller_name,
    )
    .expect("构造季前通知投递");
    repository.create_season_notice_delivery(command.clone()).await.expect("建立季前通知投递");
    repository.create_season_notice_delivery(command).await.expect("重放投递必须幂等");
    assert!(
        repository
            .season_notice_deliveries(&fixture.season)
            .await
            .expect("重扫待建投递")
            .is_empty()
    );

    let notice_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_season_notices
         WHERE position_id = $1 AND seller_user_id = $2 AND kind = 'preclose_withdrawal'",
    )
    .bind(position_id)
    .bind(seller().as_uuid())
    .fetch_one(pool)
    .await
    .expect("读取通知数");
    assert_eq!(notice_count, 1);
    let command_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_commands WHERE operation = 'season_notice'",
    )
    .fetch_one(pool)
    .await
    .expect("读取已提交命令数");
    assert_eq!(command_count, 1, "两轮排队只提交一条季通知命令");

    let (notice_id,): (Uuid,) = sqlx::query_as(
        "SELECT id FROM public.consignment_season_notices
         WHERE position_id = $1 AND seller_user_id = $2",
    )
    .bind(position_id)
    .bind(seller().as_uuid())
    .fetch_one(pool)
    .await
    .expect("读取通知身份");
    let (work_id, action, work_delivery): (Uuid, String, Uuid) = sqlx::query_as(
        "SELECT id, action, delivery_id FROM public.mail_disposition_work WHERE notice_id = $1",
    )
    .bind(notice_id)
    .fetch_one(pool)
    .await
    .expect("读取季前通知工作");
    assert_eq!(work_id, season_notice_work_id(notice_id));
    assert_eq!(action, "send");
    let work_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_disposition_work WHERE notice_id = $1",
    )
    .bind(notice_id)
    .fetch_one(pool)
    .await
    .expect("读取季前通知工作数");
    assert_eq!(work_count, 1, "同一通知只应有一份 Send 工作");
    let (delivery_id, delivery_command, origin_id, external_mail_id, payload): (
        Uuid,
        Uuid,
        String,
        String,
        serde_json::Value,
    ) = sqlx::query_as(
        "SELECT id, command_id, origin_id, payload->>'external_mail_id', payload
         FROM public.deliveries WHERE origin_kind = 'consignment_notice'",
    )
    .fetch_one(pool)
    .await
    .expect("读取季前通知投递");
    assert_eq!(delivery_id, season_notice_delivery_id(notice_id));
    assert_eq!(delivery_command, season_notice_delivery_id(notice_id));
    assert_eq!(work_delivery, delivery_id);
    assert_eq!(origin_id, format!("consignment_notice:{notice_id}"));
    assert_eq!(external_mail_id, format!("consignment-notice:{notice_id}"));
    assert_eq!(payload["operation"], "send");
    assert_eq!(payload["template"], "consignment_season_notice");
    assert_eq!(payload["world"], "isekai");
    assert_eq!(payload["credits"], 0);
    assert_eq!(payload["cod"], 0);
    assert_eq!(payload["attachment_ids"], serde_json::json!([]));
    let body = payload["body"].as_str().expect("正文必须冻结");
    assert!(body.contains(&fixture.season.end_utc.to_rfc3339_opts(SecondsFormat::AutoSi, true)));
    assert!(!body.contains("C1-"), "正文不得含预约码");
    let delivery_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.deliveries WHERE origin_kind = 'consignment_notice'",
    )
    .fetch_one(pool)
    .await
    .expect("读取投递数");
    assert_eq!(delivery_count, 1);
}

#[tokio::test]
async fn postgres_consignment_season_window_freezes_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let repository = repository(&database).await;
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let start = Timestamp::parse_from_rfc3339("2026-05-01T07:00:00Z").expect("开始");
    let end = Timestamp::parse_from_rfc3339("2026-11-01T07:00:00Z").expect("结束");
    let cutoff = Timestamp::parse_from_rfc3339("2026-10-29T07:00:00Z").expect("预停");
    let first = FrozenSeason {
        world: SettlementWorld::Isekai,
        season_id: "2026-05-01".to_owned(),
        start_utc: start,
        end_utc: end,
        admission_cutoff_utc: cutoff,
        calendar_version: CalendarVersion::from_bytes([0x31; 32]),
        preclose_days: 3,
    };
    let mut changed = first.clone();
    changed.end_utc = end.checked_add_signed(auction_time::Duration::days(2)).expect("延后");
    changed.admission_cutoff_utc =
        cutoff.checked_add_signed(auction_time::Duration::days(2)).expect("延后预停");
    changed.calendar_version = CalendarVersion::from_bytes([0x32; 32]);
    changed.preclose_days = 5;

    let stored = repository.freeze_season_window(first.clone(), "UTC").await.expect("首次冻结");
    assert_eq!(stored, first);
    let replay = repository.freeze_season_window(changed, "UTC").await.expect("同季再次冻结");
    assert_eq!(replay, first, "同一季以首次冻结为准，不因配置变化改写");
    let rows: Vec<(Timestamp, Timestamp, Timestamp, Vec<u8>, i16)> = sqlx::query_as(
        "SELECT start_utc, end_utc, admission_cutoff_utc, calendar_version, preclose_days
         FROM public.consignment_season_windows WHERE world_id = 'isekai' AND season_id = $1",
    )
    .bind("2026-05-01")
    .fetch_all(pool)
    .await
    .expect("读取冻结季窗");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, start);
    assert_eq!(rows[0].1, end);
    assert_eq!(rows[0].2, cutoff);
    assert_eq!(rows[0].3, first.calendar_version.as_bytes().to_vec());
    assert_eq!(rows[0].4, 3);

    let next = FrozenSeason {
        season_id: "2026-11-01".to_owned(),
        start_utc: end,
        end_utc: Timestamp::parse_from_rfc3339("2027-05-01T07:00:00Z").expect("下一季结束"),
        admission_cutoff_utc: Timestamp::parse_from_rfc3339("2027-04-28T07:00:00Z")
            .expect("下一季预停"),
        ..first
    };
    assert_eq!(
        repository.freeze_season_window(next.clone(), "UTC").await.expect("冻结下一季"),
        next
    );
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_season_windows WHERE world_id = 'isekai'",
    )
    .fetch_one(pool)
    .await
    .expect("读取季窗数");
    assert_eq!(total, 2, "下一季可以冻结新行");
}
