//! 自动重列（7A-2 auto_relist）：worker 的 auto_relist 命令、g+1 重列代、指针 CAS 与 relisted 事件的
//! 数据库绑定；冻结开关为关或挂单未下架时不写任何行。
//!
//! 保护的不变量：2、3（新代、指针、事件、回执同事务）、6（失败关闭）、10（冻结配置）。

use std::sync::Arc;

use auction_application::consignment::{
    AutoRelistOutcome, AutoRelistService, AutoRelistStore, PendingAutoRelist,
};
use serde_json::json;
use sqlx::PgPool;

use super::super::contracts::fixture::seed_main_custody;
use super::super::deposit::support::{LISTING, fails, succeeds};
use super::super::funding::fixture::{cash_refund_intake, consumption_sql};
use super::super::ledger136::fixture::refund_dispatch_sql;
use super::command_sql::{
    PRICE, V6_AT, command, event, generation, hold_sql, listing_update, pointer,
};
use super::seed;
use super::unpaid_recovery::{DISPOSITION as UNPAID_DISPOSITION, seed_unpaid_recovered};
use crate::support::TestDatabase;

const AUTO_COMMAND: &str = "c7e00000-0000-5000-8000-0000000000a1";
const DISPOSITION: &str = "c7d10000-0000-4000-8000-000000000001";

fn auto_command(actor: (&str, &str)) -> String {
    let body = json!({"schema_version": 2, "operation": "auto_relist", "disposition": "relisted",
        "command_id": AUTO_COMMAND, "listing_id": LISTING, "listing_version": 6,
        "state": "available", "price": PRICE, "generation": 2});
    command(
        AUTO_COMMAND,
        actor,
        "auto_relist",
        ("listing", Some(LISTING), LISTING),
        Some(LISTING),
        200,
        &body,
        V6_AT,
    )
}

fn auto_relist_sql(event_actor: &str) -> String {
    auto_command(("worker", "consignment"))
        + &generation(2, "relist", 1, AUTO_COMMAND)
        + &pointer(2, 3)
        + &event(
            6,
            AUTO_COMMAND,
            (event_actor, "relisted"),
            ("held_off_market", "available"),
            None,
            V6_AT,
        )
        + &listing_update("available", None, 6, V6_AT)
}

/// 账本只认 worker 的 auto_relist；重列事件必须与命令同一 actor，并带同事务的 g+1 重列代与指针推进。
#[tokio::test]
async fn postgres_consignment_recovery_auto_relist_binds_the_worker_command_to_the_relist_event() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &hold_sql()).await;

    // 反例：auto_relist 只允许 worker，system 命令被账本拒绝。
    fails(
        pool,
        &auto_command(("system", "consignment")),
        "23514",
        Some("consignment_command_actor"),
    )
    .await;
    // 反例：事件 actor 不是发出命令的 worker（借用卖家身份的事件）。
    fails(pool, &auto_relist_sql("public_user"), "23514", None).await;
    // 反例：命令与事件齐备，但没有 g+1 重列代与指针推进。
    fails(
        pool,
        &(auto_command(("worker", "consignment"))
            + &event(
                6,
                AUTO_COMMAND,
                ("worker", "relisted"),
                ("held_off_market", "available"),
                None,
                V6_AT,
            )
            + &listing_update("available", None, 6, V6_AT)),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;
    // 正例：命令、g+1 重列代、指针推进、事件与挂单版本同一事务。
    succeeds(pool, &auto_relist_sql("worker")).await;
    let (state, generation): (String, i64) = sqlx::query_as(
        "SELECT listing.state, position.current_generation
         FROM public.consignment_listings AS listing
         JOIN public.consignment_custody_positions AS position
           ON position.listing_id = listing.id",
    )
    .fetch_one(pool)
    .await
    .expect("读取挂单与指针");
    assert_eq!((state.as_str(), generation), ("available", 2));
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取计数")
}

/// 一份允许自动再售的未付收回处置（冻结开关由参数决定），挂在已有的消费上；
/// 以维护账号直接写入合法行，不改任何约束。
async fn seed_disposition(pool: &PgPool, enabled: bool) {
    sqlx::query(
        "INSERT INTO public.consignment_return_dispositions
            (disposition_id, consumption_id, contract_id, leg, buyer_right, refund, principal,
             listing_effect, relist, auto_relist_enabled, config_digest, command_id, decided_at)
         SELECT $1::uuid, consumption.id, consumption.contract_id, consumption.leg,
                'close_at_limit', 'none', 'none', 'held_unpaid_recovered', 'manual_or_auto', $2,
                decode(repeat('aa', 32), 'hex'),
                (SELECT command_id FROM public.consignment_commands ORDER BY command_id LIMIT 1),
                now()
         FROM public.consignment_return_consumptions AS consumption",
    )
    .bind(DISPOSITION)
    .bind(enabled)
    .execute(pool)
    .await
    .expect("写入处置");
}

/// 止售中的存量：Sale 合同在代管 position 上，挂单因运营止售下架，另有一次已消费的退回
/// （只为让处置行有消费可引）与一份允许自动再售的处置。
async fn seed_held(pool: &PgPool, enabled: bool) {
    seed(pool).await;
    succeeds(pool, &hold_sql()).await;
    succeeds(pool, &refund_dispatch_sql()).await;
    let intake = cash_refund_intake();
    succeeds(pool, &intake.sql()).await;
    succeeds(pool, &intake.start_sql()).await;
    succeeds(pool, &consumption_sql(&intake)).await;
    seed_disposition(pool, enabled).await;
}

fn pending() -> PendingAutoRelist {
    PendingAutoRelist {
        disposition_id: DISPOSITION.parse().expect("处置 ID"),
        listing_id: LISTING.parse().expect("挂单 ID"),
    }
}

/// 自动重列的两道闸：冻结开关为关不作决定；开关为开也绕不过有效的运营止售（挂单不是未付收回下架态）。
/// 两种情形挂单都留库，不建新代、不写决定、不占命令 ID，也不进待办。
#[tokio::test]
async fn postgres_consignment_recovery_auto_relist_never_overrides_frozen_off_or_an_operations_hold()
 {
    for enabled in [false, true] {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("维护账号");
        let pool = owner.postgres_pool();
        seed_held(pool, enabled).await;
        let store = Arc::new(database.store().await.expect("运行账号"));
        let port: Arc<dyn AutoRelistStore> = store;
        let service = AutoRelistService::new(port);

        assert!(service.pending(None, 10).await.expect("待办").items.is_empty());
        let receipt = service.relist(pending(), 5).await.expect("无可作决定时不是错误");
        assert_eq!(receipt.outcome, AutoRelistOutcome::NotReady);
        let (state, reason): (String, Option<String>) =
            sqlx::query_as("SELECT state, reason_code FROM public.consignment_listings")
                .fetch_one(pool)
                .await
                .expect("读取挂单");
        assert_eq!(
            (state.as_str(), reason.as_deref()),
            ("held_off_market", Some("operations_hold"))
        );
        assert_eq!(
            count(pool, "SELECT count(*) FROM public.consignment_relist_decisions").await,
            0
        );
        assert_eq!(
            count(pool, "SELECT count(*) FROM public.consignment_disposition_generations").await,
            1,
            "不新增处分代"
        );
        assert_eq!(
            count(
                pool,
                "SELECT count(*) FROM public.consignment_commands WHERE operation = 'auto_relist'"
            )
            .await,
            0
        );
    }
}

async fn listing_row(pool: &PgPool) -> (String, Option<String>, i64) {
    sqlx::query_as("SELECT state, reason_code, state_version FROM public.consignment_listings")
        .fetch_one(pool)
        .await
        .expect("读取挂单")
}

fn unpaid_pending() -> PendingAutoRelist {
    PendingAutoRelist {
        disposition_id: UNPAID_DISPOSITION.parse().expect("处置 ID"),
        listing_id: LISTING.parse().expect("挂单 ID"),
    }
}

async fn position_row(pool: &PgPool) -> (String, Option<i64>) {
    sqlx::query_as("SELECT state, current_generation FROM public.consignment_custody_positions")
        .fetch_one(pool)
        .await
        .expect("读取 position")
}

/// 冻结开关是自动重列的第一道闸（核心规则）：处置里冻结为关时，即使挂单已是 unpaid_recovered 下架、
/// Sale 已按未付取消封闭、原件在库、处置允许自动再售，也不作决定——挂单、position、决定、代、命令账本都
/// 不变，且不进待办。与下一个测试成对：同一存量把开关改为开就得到新代，所以关闭分支确实是被开关拦下的。
#[tokio::test]
async fn postgres_consignment_recovery_auto_relist_frozen_off_keeps_an_unpaid_recovered_listing() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let pool = owner.postgres_pool();
    seed_unpaid_recovered(pool, false).await;
    let store = Arc::new(database.store().await.expect("运行账号"));
    let port: Arc<dyn AutoRelistStore> = store;
    let service = AutoRelistService::new(port);

    let held = ("held_off_market".to_owned(), Some("unpaid_recovered".to_owned()), 7);
    assert_eq!(listing_row(pool).await, held);
    assert!(service.pending(None, 10).await.expect("待办").items.is_empty(), "开关为关不进待办");
    let receipt = service.relist(unpaid_pending(), 5).await.expect("无可作决定时不是错误");
    assert_eq!(receipt.outcome, AutoRelistOutcome::NotReady);
    assert_eq!(listing_row(pool).await, held, "挂单不变");
    assert_eq!(position_row(pool).await, ("committed".to_owned(), Some(1)), "position 不变");
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_relist_decisions").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.consignment_disposition_generations").await,
        1,
        "不新增处分代"
    );
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_commands WHERE operation = 'auto_relist'"
        )
        .await,
        0
    );
}

/// 自动重列的正例：冻结开关为开、挂单 unpaid_recovered 下架、Sale 已按未付取消封闭、原件从未离库，
/// 经 Rust 写端先回库（committed -> in_custody），再得到 g+1 重列代、指针 CAS、挂单回到 available、
/// relisted 事件、决定与回执；重放返回原结果。
#[tokio::test]
async fn postgres_consignment_recovery_auto_relist_frozen_on_relists_an_unpaid_recovered_listing() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let pool = owner.postgres_pool();
    seed_unpaid_recovered(pool, true).await;
    let store = Arc::new(database.store().await.expect("运行账号"));
    let port: Arc<dyn AutoRelistStore> = store;
    let service = AutoRelistService::new(port);

    assert_eq!(service.pending(None, 10).await.expect("待办").items, vec![unpaid_pending()]);
    // 键集分页：读满一页给出游标，游标之后没有更多待办（正例）；读不满一页没有游标（反例）。
    let page = service.pending(None, 1).await.expect("满页");
    assert_eq!(page.items, vec![unpaid_pending()]);
    let next = page.next.expect("读满一页应给出游标");
    let after = service.pending(Some(next), 1).await.expect("游标之后");
    assert!((after.items.is_empty(), after.next.is_none()) == (true, true), "{after:?}");
    assert_eq!(service.pending(None, 10).await.expect("未满页").next, None);
    let receipt = service.relist(unpaid_pending(), 5).await.expect("自动重列");
    let AutoRelistOutcome::Relisted { listing_id, listing_version, generation } = receipt.outcome
    else {
        panic!("开关为开的未付收回下架应重列: {:?}", receipt.outcome);
    };
    assert_eq!(
        (listing_id, listing_version, generation, receipt.replayed),
        (unpaid_pending().listing_id, 8, 2, false)
    );

    assert_eq!(listing_row(pool).await, ("available".to_owned(), None, 8));
    assert_eq!(
        position_row(pool).await,
        ("in_custody".to_owned(), Some(2)),
        "回库且指针 CAS 到 g+1"
    );
    let (purpose, predecessor): (String, i64) = sqlx::query_as(
        "SELECT purpose, predecessor FROM public.consignment_disposition_generations
         WHERE generation = 2",
    )
    .fetch_one(pool)
    .await
    .expect("g+1 重列代");
    assert_eq!((purpose.as_str(), predecessor), ("relist", 1));
    let (actor, kind): (String, String) = sqlx::query_as(
        "SELECT actor_kind, event_kind FROM public.consignment_events WHERE state_version = 8",
    )
    .fetch_one(pool)
    .await
    .expect("relisted 事件");
    assert_eq!((actor.as_str(), kind.as_str()), ("worker", "relisted"));
    let recall: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_custody_events WHERE event_kind = 'uncommitted'",
    )
    .fetch_one(pool)
    .await
    .expect("回库事件");
    assert_eq!(recall, 1);
    let (outcome, new_generation): (String, Option<i64>) =
        sqlx::query_as("SELECT outcome, new_generation FROM public.consignment_relist_decisions")
            .fetch_one(pool)
            .await
            .expect("决定");
    assert_eq!((outcome.as_str(), new_generation), ("relisted", Some(2)));
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_commands WHERE operation = 'auto_relist'"
        )
        .await,
        1,
        "成功回执入账"
    );
    assert!(service.pending(None, 10).await.expect("待办").items.is_empty(), "已有决定不再待办");

    let replay = service.relist(unpaid_pending(), 5).await.expect("重放");
    assert!(replay.replayed);
    assert_eq!(replay.outcome, receipt.outcome);
    assert_eq!(listing_row(pool).await, ("available".to_owned(), None, 8), "重放不改库");
}
