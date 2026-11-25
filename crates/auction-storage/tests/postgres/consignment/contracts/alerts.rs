//! 义务表读侧的寄售来源过滤：告警、巡检、SLI 与 Admin 只读视图不把寄售夹具当作拍卖卡住或拍卖义务
//! 展示；需要人工的寄售履约状态仍对运营可见。

use std::assert_matches;

use auction_application::consignment::{ConsignmentFulfillmentClaims, ConsignmentGraphStore};
use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryAdminError, DeliveryAdminPort, PlayerSupportRequest, PlayerSupportSelector,
};
use auction_application::operations::{
    OperationsAlertCollectionOverdueRead, OperationsAlertFulfillmentSnapshot,
    OperationsAlertSnapshot, OperationsAlertSourceReader,
};
use auction_application::sli::SliSnapshotReader;
use auction_domain::SettlementWorld;
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::{USER_2, succeeds};
use super::fixture::{seed_isekai_custody, seed_main_custody};
use super::graph_write::{graph_rows, sale_graph, seed_funding};
use crate::support::TestDatabase;
use crate::support::fixtures::{policy_snapshot, stable_id};

/// 写入合同图后的测试库：owner 做夹具与破坏，runtime 执行被测端口。
struct Seeded {
    _database: TestDatabase,
    owner: DatabaseStore,
    runtime: DatabaseStore,
}

impl Seeded {
    async fn sale(world: SettlementWorld) -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let runtime = database.store().await.expect("以运行账号连接");
        let pool = owner.postgres_pool();
        if world == SettlementWorld::Isekai {
            seed_isekai_custody(pool, "transaction_timestamp() + interval '10 days'").await;
        } else {
            seed_main_custody(pool).await;
        }
        seed_funding(pool).await;
        runtime.write_graph(sale_graph(world, 1000)).await.expect("写入合同图");
        assert!(graph_rows(pool).await.2 >= 3, "合同图义务已写入");
        Self { _database: database, owner, runtime }
    }

    fn pool(&self) -> &PgPool {
        self.owner.postgres_pool()
    }

    async fn alerts(&self) -> OperationsAlertSnapshot {
        self.runtime
            .snapshot(Timestamp::now(), OperationsAlertCollectionOverdueRead::Skip)
            .await
            .expect("读取告警快照")
    }
}

/// (积压, 阻塞, 失败, 人工对账, 等待付款, 等待前置, 等待领取)
fn waterline(snapshot: OperationsAlertSnapshot) -> (i64, i64, i64, i64, i64, i64, i64) {
    let OperationsAlertFulfillmentSnapshot {
        pending,
        blocked,
        failed,
        manual_reconcile,
        waiting_payment,
        waiting_prerequisite,
        awaiting_collection,
        ..
    } = snapshot.fulfillment.expect("履约水位");
    (
        pending,
        blocked,
        failed,
        manual_reconcile,
        waiting_payment,
        waiting_prerequisite,
        awaiting_collection,
    )
}

#[tokio::test]
async fn postgres_consignment_graph_is_not_counted_as_auction_fulfillment_backlog() {
    let seeded = Seeded::sale(SettlementWorld::Main).await;
    // 付款腿 ready/pending、卖款 blocked 且前置未满足：按拍卖口径会是 1 积压、1 阻塞。
    assert_eq!(waterline(seeded.alerts().await), (0, 0, 0, 0, 0, 0, 0));

    // 来源损坏被寄售领取转人工：需要人工的状态对运营可见，但仍不计入拍卖积压 / 阻塞。
    succeeds(
        seeded.pool(),
        "UPDATE public.settlement_obligation_fulfillments SET dispatch_world = 'isekai'
         WHERE obligation_id IN (SELECT id FROM public.settlement_obligations
                                 WHERE obligation_kind = 'winner_equipment_cod');",
    )
    .await;
    let now = Timestamp::now();
    let claims = seeded
        .runtime
        .claim_consignment_obligations(
            now,
            "consignment-alert-test",
            now + auction_time::Duration::minutes(5),
            10,
        )
        .await
        .expect("寄售领取");
    assert!(claims.is_empty(), "损坏来源不被领取");
    assert_eq!(waterline(seeded.alerts().await), (0, 0, 0, 1, 0, 0, 0));
}

#[tokio::test]
async fn postgres_consignment_obligations_are_not_season_pending_auction_obligations() {
    let seeded = Seeded::sale(SettlementWorld::Isekai).await;
    // 前提：冻结 isekai 换季边界前七天内存在未结算的非或有义务；按拍卖口径它们会被计入。
    let now = Timestamp::now();
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, intake_opens_at,
             policy_snapshot)
         VALUES ($1, 'isekai', 1, 'season boundary', 'settled', $2, $2, $2, $2, $2, $2, $3, $4)",
    )
    .bind(stable_id("consignment-alerts", "auction"))
    .bind(now - auction_time::Duration::days(10))
    .bind(now + auction_time::Duration::days(3))
    .bind(policy_snapshot(now - auction_time::Duration::days(10), 50_000, 30_000))
    .execute(seeded.pool())
    .await
    .expect("写入换季边界场次");
    let unfiltered: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_obligations
         WHERE world_id = 'isekai' AND business_state <> 'contingent'",
    )
    .fetch_one(seeded.pool())
    .await
    .expect("寄售义务数");
    assert_eq!(unfiltered, 3);

    let lifecycle = seeded.alerts().await.lifecycle.expect("生命周期水位");
    assert_eq!(lifecycle.season_obligations_pending, 0);
}

#[tokio::test]
async fn postgres_consignment_graph_passes_consistency_inspection_and_sli() {
    let seeded = Seeded::sale(SettlementWorld::Main).await;
    let database_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(seeded.pool())
        .await
        .expect("数据库名");

    let report = seeded.runtime.inspect_consistency(&database_name).await.expect("巡检");
    let violations: Vec<_> = report
        .checks
        .iter()
        .flat_map(|check| check.samples.iter().map(|sample| (check.invariant, sample)))
        .collect();
    assert!(report.complete && report.consistent, "寄售夹具不得产生拍卖口径的违规：{violations:?}");

    // SLI 只借空闲连接，空闲池空时按设计返回 Busy。sqlx 在连接 drop 后由后台任务做 ping 再归还，
    // 查询返回并不意味着连接已空闲；因此预热后等到池里确有空闲连接，再读快照。
    let pool = seeded.runtime.postgres_pool();
    sqlx::query("SELECT 1").execute(pool).await.expect("预热连接");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while pool.num_idle() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("预热连接应归还到空闲池");
    seeded.runtime.read_sli_snapshot().await.expect("SLI 快照不因寄售义务失败");
}

#[tokio::test]
async fn postgres_consignment_obligations_stay_out_of_auction_admin_views() {
    let seeded = Seeded::sale(SettlementWorld::Main).await;

    // 玩家支持汇总：买家（付款腿收款人）名下的寄售义务不出现，也不能让解码失败。
    let bundle = DeliveryAdminPort::player_support(
        &seeded.runtime,
        PlayerSupportRequest {
            selector: PlayerSupportSelector::User(USER_2.to_owned()),
            include_settlement_return_actions: false,
        },
    )
    .await
    .expect("读取玩家支持汇总");
    assert!(bundle.obligations.is_empty(), "寄售义务不是拍卖义务：{:?}", bundle.obligations);

    // 投递详情：来源是寄售义务时失败关闭，不把它投影成拍卖义务。
    let delivery = Uuid::parse_str("c5d00000-0000-4000-8000-000000000001").expect("夹具 UUID");
    let payment: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.settlement_obligations WHERE obligation_kind = 'winner_equipment_cod'",
    )
    .fetch_one(seeded.pool())
    .await
    .expect("付款腿义务 ID");
    succeeds(
        seeded.pool(),
        &format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('{delivery}', 'c5d00000-0000-4000-8000-000000000002',
                     decode(repeat('18', 32), 'hex'), 'settlement', '{payment}',
                     'FakeSellerTwo', '{{}}'::jsonb, 'prepared', 0, 0,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');"
        ),
    )
    .await;
    let detail = DeliveryAdminPort::detail(&seeded.runtime, delivery).await;
    assert_matches!(
        detail,
        Err(ApplicationError::DeliveryAdmin(error)) if error == DeliveryAdminError::Unavailable
    );
}
