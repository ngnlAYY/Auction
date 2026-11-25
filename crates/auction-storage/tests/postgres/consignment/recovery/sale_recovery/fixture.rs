//! 7C-3 测试共用的夹具：真实写端取得并上架的 `World`、已售止售存量、封闭事实与未知退款派发的 SQL。
//!
//! 自 `mod.rs` 析出，使 `postgres_audit` 目标能以符号链接复用而不带入本目录的测试模块。

use std::sync::Arc;

use auction_application::consignment::ConsignmentError;
use auction_application::settlement::{
    ConsignmentFundingPreflight, FundingFuture, FundingPreflightFact,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::funding::FundingMode;
use auction_domain::consignment::{ConsignmentSalePolicy, FeeBps, ListingPrice};
use auction_storage::{
    DepositRepository, DepositSettings, FirstDispatchContent, FirstDispatchContentSource,
    FirstDispatchRequest, PurchaseRepository, WithdrawalDispatchRequest,
};
use auction_time::Timestamp;
use serde_json::json;
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use super::super::super::contracts::fixture::CONTRACT;
use super::super::super::deposit::support::{
    ATTEMPT, DECISION, LISTING, POSITION, PROOF, PROOF_COMMAND, USER_1, seed_accepted, succeeds,
};
use super::super::super::ledger136::fixture::sale_graph;
use super::super::command_sql::{V6_AT, command, event, listing_update};
use crate::support::TestDatabase;

/// 夹具止售命令：同时是存量 Sale 的来源命令（每份合同一条来源命令）。
pub(in crate::consignment::recovery) const HOLD: &str = "c7f00000-0000-4000-8000-000000000001";
const HOLD_ID: &str = "c7f10000-0000-4000-8000-000000000001";
const BOUNDARY_SOURCE: &str = "c7f20000-0000-4000-8000-000000000001";
/// 第二位买家（与存量 Sale 的买家不同）。
pub(in crate::consignment::recovery) const BUYER_3: &str = "10000000-0000-0000-0000-000000000003";
pub(in crate::consignment::recovery) const PRICE: i64 = 101;

/// 玩家 UUIDv4 命令 ID。
pub(in crate::consignment::recovery) fn command_id(seed: u128) -> auction_domain::CommandId {
    auction_domain::CommandId::from_uuid(Uuid::from_u128(
        0x7cf0_0000_0000_4000_8000_0000_0000_0000 + seed,
    ))
}

pub(in crate::consignment::recovery) fn id(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("夹具 UUID")
}

pub(in crate::consignment::recovery) fn seller() -> auction_domain::UserId {
    auction_domain::UserId::from_uuid(id(USER_1))
}

/// 资金预检恒 Ready：只服务第二位买家的购买。
struct ReadyPreflight;

impl ConsignmentFundingPreflight for ReadyPreflight {
    fn preflight(&self, _world: SettlementWorld) -> FundingFuture<'_, FundingPreflightFact> {
        Box::pin(async {
            Ok(FundingPreflightFact::Ready {
                uid: "7001".to_owned(),
                mode: FundingMode::StrictCash,
                boundary_at: Timestamp::parse_from_rfc3339("2026-01-02T00:00:00Z").expect("时刻"),
                boundary_source_id: Some(id(BOUNDARY_SOURCE)),
                account_version: 0,
            })
        })
    }
}

/// 假首轮内容：明显的假文本与固定摘要。
pub(super) struct FakeContent;

impl FirstDispatchContentSource for FakeContent {
    fn content(
        &self,
        request: &FirstDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        Ok(FirstDispatchContent {
            template: "consignment_cod".to_owned(),
            subject: "fake subject".to_owned(),
            body_digest: [0xd1; 32],
            equipment_ids: vec![request.canonical_eid.to_owned()],
        })
    }

    fn withdrawal_content(
        &self,
        request: &WithdrawalDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        Ok(FirstDispatchContent {
            template: "consignment_equipment_withdrawal".to_owned(),
            subject: "fake withdrawal subject".to_owned(),
            body_digest: [0xd2; 32],
            equipment_ids: vec![request.canonical_eid.to_owned()],
        })
    }
}

/// 封闭事实的种类。
#[derive(Clone, Copy)]
pub(in crate::consignment::recovery) enum Closure {
    /// 不写封闭事实：Sale 仍占着处分代。
    None,
    /// 买家从未付款，购买被取消。
    CancelledUnpaid,
}

pub(in crate::consignment::recovery) struct World {
    _database: TestDatabase,
    pub(in crate::consignment::recovery) owner: PgPool,
    pub(in crate::consignment::recovery) pool: PgPool,
    pub(in crate::consignment::recovery) deposit: DepositRepository,
    pub(in crate::consignment::recovery) purchases: PurchaseRepository,
}

impl World {
    /// 真实写端：整封取得后挂单 available（版本 5），position 在库、处分代 1；闸门打开，资金账户就绪。
    pub(in crate::consignment::recovery) async fn taken() -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let runtime = database.store().await.expect("运行账号");
        let pool = runtime.postgres_pool().clone();
        seed_accepted(&pool).await;
        let owner = database.maintenance_store().await.expect("维护账号").postgres_pool().clone();
        succeeds(
            &owner,
            &format!(
                "UPDATE public.consignment_lookup_control
                 SET admitting = true, enabled_epoch = 1,
                     worker_ready_until = clock_timestamp() + interval '1 hour' WHERE id = 1;
                 INSERT INTO public.settlement_funding_accounts (uid, status)
                 VALUES ('7001', 'ready') ON CONFLICT DO NOTHING;
                 INSERT INTO public.settlement_funding_sources
                    (id, uid, kind, source_key, amount, proof_digest)
                 VALUES ('{BOUNDARY_SOURCE}', '7001', 'reconciliation',
                         'c7f20000-0000-4000-8000-0000000000f1', 0,
                         decode(repeat('a1', 32), 'hex'));
                 INSERT INTO public.users (id, username, public_key_hash)
                 VALUES ('{BUYER_3}', 'FakeBuyerThree', decode(repeat('15', 32), 'hex'));"
            ),
        )
        .await;
        let deposit = DepositRepository::without_reservation_crypto(
            runtime.clone(),
            DepositSettings::new(
                FeeBps::new(500).expect("费率"),
                ListingPrice::new(ListingPrice::MIN).expect("下限"),
                ListingPrice::new(ListingPrice::MAX).expect("上限"),
                24,
                100,
            ),
        )
        .with_dispatch_content(Arc::new(FakeContent));
        let purchases = PurchaseRepository::new(
            runtime,
            Arc::new(ReadyPreflight),
            Arc::new(FakeContent),
            ConsignmentSalePolicy::new(7, 3, 5).expect("策略"),
        );
        let world = Self { _database: database, owner, pool, deposit, purchases };
        world
            .deposit
            .test_apply_take(id(PROOF_COMMAND), id(DECISION), id(PROOF), id(ATTEMPT), id(LISTING))
            .await
            .expect("取得");
        world
    }

    /// 取得后再写入“已售、止售、position committed”的存量；`closure` 决定 Sale 是否已封闭。
    pub(in crate::consignment::recovery) async fn sold(closure: Closure) -> Self {
        let world = Self::taken().await;
        let at = database_time(&world.owner).await;
        succeeds(&world.owner, &sold_and_held_sql(&at)).await;
        if matches!(closure, Closure::CancelledUnpaid) {
            succeeds(&world.owner, &closure_sql("cancelled_unpaid")).await;
        }
        world
    }

    pub(in crate::consignment::recovery) async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM public.{table}")))
            .fetch_one(&self.pool)
            .await
            .expect("计数")
    }

    pub(in crate::consignment::recovery) async fn listing(&self) -> (String, i64) {
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id = $1")
            .bind(id(LISTING))
            .fetch_one(&self.pool)
            .await
            .expect("读取挂单")
    }

    /// position 的 (状态, 当前代, 版本)。
    pub(in crate::consignment::recovery) async fn position(&self) -> (String, Option<i64>, i64) {
        sqlx::query_as(
            "SELECT state, current_generation, version
             FROM public.consignment_custody_positions WHERE id = $1",
        )
        .bind(id(POSITION))
        .fetch_one(&self.pool)
        .await
        .expect("读取 position")
    }

    /// 处分代按序号排序的 (序号, 用途, 前驱)。
    pub(in crate::consignment::recovery) async fn generations(
        &self,
    ) -> Vec<(i64, String, Option<i64>)> {
        sqlx::query_as(
            "SELECT generation, purpose, predecessor
             FROM public.consignment_disposition_generations
             WHERE position_id = $1 ORDER BY generation",
        )
        .bind(id(POSITION))
        .fetch_all(&self.pool)
        .await
        .expect("读取处分代")
    }

    /// 挂单事件种类出现的次数。
    pub(in crate::consignment::recovery) async fn listing_events(&self, kind: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM public.consignment_events WHERE listing_id = $1 AND event_kind = $2",
        )
        .bind(id(LISTING))
        .bind(kind)
        .fetch_one(&self.pool)
        .await
        .expect("读取挂单事件")
    }
}

/// 数据库当前时刻（UTC 文本）：存量的 updated_at 必须不早于真实写端留下的时刻。
pub(in crate::consignment::recovery) async fn database_time(pool: &PgPool) -> String {
    sqlx::query_scalar(
        "SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
    )
    .fetch_one(pool)
    .await
    .expect("数据库时刻")
}

/// 止售（available -> held_off_market/operations_hold 版本 6；现有数据库只允许 Admin 以 operations_hold 写 hold_set，控制行不建，所以不构成“有效止售”）、Sale 占处分代 1、position 在库 -> committed（版本 3）。
fn sold_and_held_sql(at: &str) -> String {
    let body = json!({"schema_version": 2, "operation": "admin_delist",
        "disposition": "hold_set", "command_id": HOLD, "listing_id": LISTING,
        "listing_version": 6, "state": "held_off_market", "held_reason": "operations_hold",
        "hold_id": HOLD_ID});
    command(
        HOLD,
        ("admin", "fake-admin"),
        "admin_delist",
        ("listing", Some(LISTING), LISTING),
        Some(LISTING),
        200,
        &body,
        at,
    ) + &event(
        6,
        HOLD,
        ("admin", "hold_set"),
        ("available", "held_off_market"),
        Some("operations_hold"),
        at,
    ) + &listing_update("held_off_market", Some("operations_hold"), 6, at)
        + &sale_graph().sql(HOLD)
        + &format!(
            "UPDATE public.consignment_custody_positions
             SET state = 'committed', version = 3, updated_at = '{at}'
             WHERE id = '{POSITION}';
             INSERT INTO public.consignment_custody_events
                (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
                 listing_id, listing_version, occurred_at)
             VALUES ('{POSITION}', 3, '{HOLD}', 'committed', 'in_custody', 'committed', NULL,
                     '{LISTING}', 6, '{at}');"
        )
}

/// 封闭事实：存量 Sale 的封闭（只追加）。
pub(in crate::consignment::recovery) fn closure_sql(kind: &str) -> String {
    format!(
        "INSERT INTO public.consignment_sale_closures (contract_id, closure_kind, basis_ref, closed_at)
         VALUES ('{CONTRACT}', '{kind}', 'fake-disposition', '{V6_AT}');"
    )
}

/// 买家退款腿的派发已起写（Send 尝试）但没有 Sent MID 绑定：结果未知（现金腿，与原件腿无关）。
pub(in crate::consignment::recovery) fn unknown_refund_sql() -> String {
    use super::super::super::contracts::fixture::OBLIGATION_3;
    use super::super::super::ledger136::fixture::{
        PURCHASED_AT, REFUND_ATTEMPT, REFUND_DELIVERY, REFUND_ROUND, round_sql,
    };
    let command = format!("c71{}", &REFUND_DELIVERY[3..]);
    format!(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, created_at, updated_at)
         VALUES ('{REFUND_DELIVERY}', '{command}', decode(repeat('c1', 32), 'hex'), 'settlement',
                 '{OBLIGATION_3}', 'FakeSellerTwo', '{{}}'::jsonb, 'prepared', 0, 0,
                 '{PURCHASED_AT}', '{PURCHASED_AT}');
         INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         VALUES ('{REFUND_DELIVERY}', 1, '{command}', decode(repeat('c2', 32), 'hex'), 'main',
                 '{PURCHASED_AT}',
                 jsonb_build_object('world_id', 'main', 'recipient', 'FakeSellerTwo',
                     'subject', 'fake', 'body_digest',
                     (SELECT jsonb_agg(0) FROM generate_series(1, 32)), 'credits', 0,
                     'cod', 0, 'equipment_ids', '[]'::jsonb, 'credential', 'null'::jsonb));"
    ) + &round_sql(REFUND_ROUND, REFUND_DELIVERY, OBLIGATION_3, REFUND_ATTEMPT, "cash_refund")
}
