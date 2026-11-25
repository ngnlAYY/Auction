//! record_only 载体本金首封、世界与收款腿互校：isekai 载体义务的夹具与正反例。
//!
//! 夹具把 isekai 的寄售成交冻结为 record_only 账户，并写入载体付款义务在 main 的 1 Credit 冻结派发轮。

use auction_application::settlement::{CashLeg, CashPhase, FundingError, SettleCash};
use auction_domain::consignment::funding::{FundingMode, PrincipalConclusion};
use sqlx::PgPool;

use super::super::contracts::fixture::{CONTRACT, Graph, OBLIGATION_1, seed_isekai_custody};
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{
    PRICE, PURCHASE_COMMAND, PURCHASED_AT, available_sql, purchase_command, purchase_listing,
    purchase_position, seed_purchased, send_sql,
};
use super::fixture::{UID, entries_of, id, ledger_counts, mutation};
use crate::support::TestDatabase;

const WINDOW_END: &str = "transaction_timestamp() + interval '10 days'";
pub(super) const CARRIER_DELIVERY: &str = "c6520000-0000-4000-8000-000000000001";
const CARRIER_MID: &str = "9101";
const CARRIER_ROUND: &str = "c6620000-0000-4000-8000-000000000001";
const CARRIER_ATTEMPT: &str = "c6720000-0000-4000-8000-000000000001";
/// 载体 COD：买家应付 P 加载体 1 Credit。
const CARRIER_COD: i64 = PRICE + 1;

/// 载体付款派发的形状；缺省是合法形状，反例各改一处。
pub(super) struct Dispatch {
    payload_world: &'static str,
    cod: i64,
}

impl Dispatch {
    pub(super) const GOOD: Self = Self { payload_world: "main", cod: CARRIER_COD };

    fn sql(&self) -> String {
        let payload = format!(
            "{{\"schema_version\":1,\"operation\":\"send\",\"world\":\"{world}\",
              \"template\":\"settlement_payment_carrier\",\"credits\":1,\"cod\":{cod},
              \"variables\":{{\"reference\":\"{OBLIGATION_1}\",\"amount\":\"{cod}\"}},
              \"equipment_secret_refs\":[]}}",
            world = self.payload_world,
            cod = self.cod
        );
        send_sql(CARRIER_DELIVERY, OBLIGATION_1, CARRIER_MID, self.cod)
            .replace("'{}'::jsonb", &format!("'{payload}'::jsonb"))
            + &format!(
                "INSERT INTO public.consignment_dispatch_rounds
                    (contract_id, obligation_id, round, round_id, delivery_id,
                     rights_holder_identity, bot_uid, world_id, subject, template, body_digest,
                     round_ref, d0, credits, cod, prior_round, authorized_by_intake,
                     authorized_by_consumption)
                 VALUES ('{CONTRACT}', '{OBLIGATION_1}', 1, '{CARRIER_ROUND}',
                         '{CARRIER_DELIVERY}', 'FakeSellerTwo', '12345', 'main', 'fake subject',
                         'consignment_cod', decode(repeat('d1', 32), 'hex'), '{CARRIER_ROUND}',
                         '{PURCHASED_AT}', 1, {cod}, NULL, NULL, NULL);
                 INSERT INTO public.consignment_dispatch_attempts
                    (delivery_id, fence, attempt_id, contract_id, obligation_id, round, leg,
                     started_at)
                 VALUES ('{CARRIER_DELIVERY}', 1, '{CARRIER_ATTEMPT}', '{CONTRACT}',
                         '{OBLIGATION_1}', 1, 'carrier', '{PURCHASED_AT}');",
                cod = self.cod
            )
    }
}

/// isekai 已购成交：账户与成交都冻结为 record_only，载体付款义务带冻结派发轮。
pub(super) async fn seed_record_only_carrier(pool: &PgPool, dispatch: &Dispatch) {
    seed_isekai_custody(pool, WINDOW_END).await;
    succeeds(pool, &available_sql()).await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.settlement_funding_accounts (uid, status, accounting_mode)
             VALUES ('{UID}', 'ready', 'record_only');"
        ),
    )
    .await;
    let graph = Graph {
        price: PRICE,
        fee: 5,
        seller_amount: 96,
        payout_amount: 96,
        ..Graph::sale_isekai()
    }
    .sql(PURCHASE_COMMAND)
    .replace("'strict_cash'", "'record_only'");
    let command = purchase_command().replace("\"world\":\"main\"", "\"world\":\"isekai\"");
    succeeds(pool, &(command + &graph + &purchase_position() + &purchase_listing())).await;
    succeeds(pool, &dispatch.sql()).await;
}

pub(super) fn carrier_cash(byte: u8, phase: CashPhase) -> SettleCash {
    SettleCash {
        mutation: mutation(byte),
        contract_id: id(CONTRACT),
        obligation_id: id(OBLIGATION_1),
        delivery_id: id(CARRIER_DELIVERY),
        leg: CashLeg::CarrierPrincipal,
        phase,
        amount: 1,
    }
}

async fn carrier_allocation(pool: &PgPool) -> (String, i64) {
    sqlx::query_as(
        "SELECT state, amount FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = 'carrier'",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("读取载体分配")
}

async fn record_carrier_sources(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as(
        "SELECT count(*), COALESCE(sum(issued_amount), 0)::bigint
         FROM public.settlement_funding_sources WHERE uid = $1 AND kind = 'record_carrier'",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("读取载体来源")
}

#[tokio::test]
async fn postgres_consignment_funding_record_only_carrier_principal_first_seal_issues_one() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_record_only_carrier(pool, &Dispatch::GOOD).await;

    // 正例：record_only 无个人余额，首封由 record_carrier 向 carrier 发行 1，随即预留。
    let reserved =
        store.test_settle_cash(carrier_cash(1, CashPhase::Reserve)).await.expect("载体本金首封");
    assert!(!reserved.replayed);
    assert_eq!(carrier_allocation(pool).await, ("reserved".to_owned(), 1));
    assert_eq!(record_carrier_sources(pool).await, (1, 1));
    assert_eq!(entries_of(pool, UID, "issue").await, (1, 1));
    assert_eq!(entries_of(pool, UID, "transfer").await, (0, 0), "不借用任何自有余额");
    // 同命令重放不再发行；第二条命令不能再预留（同一来源只发行一次）。
    assert!(
        store.test_settle_cash(carrier_cash(1, CashPhase::Reserve)).await.expect("重放").replayed
    );
    let twice = store.test_settle_cash(carrier_cash(9, CashPhase::Reserve)).await;
    assert!(twice.is_err());
    assert_eq!(record_carrier_sources(pool).await, (1, 1));
    // 之后与严格现金同一条状态线。
    store.test_settle_cash(carrier_cash(2, CashPhase::InFlight)).await.expect("起写");
    store.test_settle_cash(carrier_cash(3, CashPhase::Spent)).await.expect("已证外达");
    assert_eq!(carrier_allocation(pool).await, ("spent".to_owned(), 1));
}

#[tokio::test]
async fn postgres_consignment_funding_record_only_carrier_rejects_wrong_source_shape() {
    // 反例：派发正文的世界不是 main、COD 不等于 P+1；库内校验拒绝，整笔回滚，不留来源、分录或预留。
    for dispatch in [
        Dispatch { payload_world: "isekai", cod: CARRIER_COD },
        Dispatch { payload_world: "main", cod: PRICE },
    ] {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let pool = owner.postgres_pool();
        let store = database.store().await.expect("以运行账号连接");
        seed_record_only_carrier(pool, &dispatch).await;
        let before = ledger_counts(pool, UID).await;
        let result = store.test_settle_cash(carrier_cash(1, CashPhase::Reserve)).await;
        assert!(result.is_err(), "{}", dispatch.cod);
        assert_eq!(record_carrier_sources(pool).await, (0, 0));
        assert_eq!(ledger_counts(pool, UID).await, before);
        assert_eq!(carrier_allocation_count(pool).await, 0, "回滚后不留分配");
    }
}

async fn carrier_allocation_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = 'carrier'",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("读取载体分配数")
}

/// 把载体义务改成 main 世界，造出“世界与收款腿不符”的存量：库内形状约束本就不允许，
/// 夹具在一次性测试库里先摘掉形状约束、世界外键与触发器再改（防御纵深的检查只能这样触达）。
pub(super) async fn force_carrier_obligation_into_main(pool: &PgPool) {
    succeeds(
        pool,
        &format!(
            "ALTER TABLE public.settlement_obligations DISABLE TRIGGER USER;
             ALTER TABLE public.settlement_obligations
                 DROP CONSTRAINT settlement_obligations_kind_shape_check,
                 DROP CONSTRAINT c5_obligation_world_fk;
             UPDATE public.settlement_obligations SET world_id = 'main'
             WHERE id = '{OBLIGATION_1}';
             ALTER TABLE public.settlement_obligations ENABLE TRIGGER USER;"
        ),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_funding_principal_fact_rejects_the_original_leg_of_main() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let store = database.store().await.expect("以运行账号连接");
    seed_purchased(owner.postgres_pool()).await;
    // 反例：main 的原件付款没有载体本金；不写任何结论。
    let result = store
        .test_record_principal_fact(
            id(CONTRACT),
            id(OBLIGATION_1),
            id(super::super::ledger136::fixture::PAY_DELIVERY),
            PrincipalConclusion::RtsPrincipal,
            FundingMode::StrictCash,
        )
        .await;
    assert!(matches!(result, Err(FundingError::Invalid)));
    let facts: i64 = sqlx::query_scalar("SELECT count(*) FROM public.consignment_principal_facts")
        .fetch_one(owner.postgres_pool())
        .await
        .expect("读取本金结论数");
    assert_eq!(facts, 0);
}

#[tokio::test]
async fn postgres_consignment_funding_world_and_cod_leg_must_match_on_every_write_path() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_record_only_carrier(pool, &Dispatch::GOOD).await;

    // 正例：isekai 的载体腿记本金结论；同结论重放为幂等，相反结论冲突。
    let record = |conclusion| {
        store.test_record_principal_fact(
            id(CONTRACT),
            id(OBLIGATION_1),
            id(CARRIER_DELIVERY),
            conclusion,
            FundingMode::RecordOnly,
        )
    };
    assert!(matches!(record(PrincipalConclusion::RtsPrincipal).await, Ok(true)));
    assert!(matches!(record(PrincipalConclusion::RtsPrincipal).await, Ok(false)));
    assert!(matches!(
        record(PrincipalConclusion::PaymentPrincipal).await,
        Err(FundingError::Conflict)
    ));

    // 反例：载体义务落在 main 世界（世界与载体腿不符），本金与现金腿两条写口都失败关闭。
    let before = ledger_counts(pool, UID).await;
    force_carrier_obligation_into_main(pool).await;
    let principal = record(PrincipalConclusion::RtsPrincipal).await;
    assert!(matches!(principal, Err(FundingError::Invalid)));
    let cash = store.test_settle_cash(carrier_cash(1, CashPhase::Reserve)).await;
    assert!(matches!(cash, Err(FundingError::Invalid)));
    assert_eq!(ledger_counts(pool, UID).await, before, "整笔回滚，账本不动");
    assert_eq!(carrier_allocation_count(pool).await, 0);
    assert_eq!(record_carrier_sources(pool).await, (0, 0));
}
