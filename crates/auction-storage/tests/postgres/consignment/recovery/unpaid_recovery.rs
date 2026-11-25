//! 未付收回处置的效果（7A-2）：`cancel_unpaid` 资金写端与挂单边 `sale_pending → held_off_market(unpaid_recovered)`。
//!
//! 保护的不变量：2、3（挂单版本、事件、回执、处置行与封闭事实同事务）、4（已付款的 Sale 不得按未付取消）、
//! 6（失败关闭）、8（封闭只追加）。夹具只写合法行，不改任何约束。

use std::sync::Arc;

use auction_application::consignment::{
    AutoRelistOutcome, AutoRelistService, AutoRelistSetting, AutoRelistStore, ConsignmentError,
    DepositBusinessRejection, PendingAutoRelist, ResolveReturn,
};
use auction_application::core::RequestHash;
use auction_application::settlement::FundingError;
use auction_domain::CommandId;
use auction_domain::consignment::recovery::{
    BuyerRightEffect, ListingEffect, PrincipalEffect, RecoveryDomainError, RefundEffect,
    RelistEligibility, ReturnDispositionDecision, ReturnDispositionFacts,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::CONTRACT;
use super::super::deposit::support::{LISTING, fails, succeeds};
use super::super::ledger136::fixture::{Intake, refund_dispatch_sql};
use super::super::ledger136::funding::{pay, seed as seed_unpaid};
use super::super::ledger136::returns::{
    CONSUMPTION, consumption_sql, proof_sql, seed as seed_refund,
};
use super::command_sql::{V7_AT, command, event, listing_update};
use crate::support::TestDatabase;

pub(in crate::consignment) const DISPOSITION: &str = "c7d20000-0000-4000-8000-000000000001";
const OTHER_DISPOSITION: &str = "c7d20000-0000-4000-8000-000000000002";
const RESOLVE_COMMAND: &str = "c7e00000-0000-5000-8000-0000000000b1";

fn intake() -> Intake {
    Intake { n: 1, leg: "cash_refund" }
}

/// 一次已消费的退回（只为让处置行有消费可引），在 [`seed_refund`] 的成交之上。
pub(in crate::consignment) async fn seed_consumption(pool: &PgPool) {
    let intake = intake();
    succeeds(pool, &intake.sql()).await;
    succeeds(pool, &intake.start_sql()).await;
    succeeds(pool, &(proof_sql(&intake) + &consumption_sql(&intake, CONSUMPTION))).await;
}

/// 合法的处置行（直接以维护账号写入）；`buyer_right` 与 `listing_effect` 由参数决定，命令外键指向任一已有命令。
pub(in crate::consignment) fn disposition_sql(
    id: &str,
    (buyer_right, listing_effect, relist): (&str, &str, &str),
    enabled: bool,
) -> String {
    format!(
        "INSERT INTO public.consignment_return_dispositions
            (disposition_id, consumption_id, contract_id, leg, buyer_right, refund, principal,
             listing_effect, relist, auto_relist_enabled, config_digest, command_id, decided_at)
         SELECT '{id}', consumption.id, consumption.contract_id, consumption.leg,
                '{buyer_right}', 'none', 'none', '{listing_effect}', '{relist}', {enabled},
                decode(repeat('aa', 32), 'hex'),
                (SELECT command_id FROM public.consignment_commands ORDER BY command_id LIMIT 1),
                now()
         FROM public.consignment_return_consumptions AS consumption;"
    )
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取计数")
}

fn uuid(text: &str) -> Uuid {
    text.parse().expect("UUID")
}

/// cancel_unpaid 的反例：没有处置，或处置不是“关闭原买方交割”（保留合同），都不得封闭。
#[tokio::test]
async fn postgres_consignment_recovery_cancel_unpaid_needs_a_closing_disposition() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let runtime = database.store().await.expect("运行账号");
    let pool = owner.postgres_pool();
    seed_refund(pool).await;
    seed_consumption(pool).await;
    let contract = uuid(CONTRACT);

    let denied = runtime.test_cancel_unpaid("", contract, uuid(DISPOSITION)).await;
    assert!(denied.is_err(), "没有处置不得封闭: {denied:?}");
    succeeds(pool, &disposition_sql(DISPOSITION, ("retain", "unchanged", "forbidden"), true)).await;
    let retained = runtime.test_cancel_unpaid("", contract, uuid(DISPOSITION)).await;
    assert!(retained.is_err(), "保留合同的处置不得封闭: {retained:?}");
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_sale_closures").await, 0);
}

/// cancel_unpaid 的正例：写 Sale 封闭事实（`cancelled_unpaid`，依据是处置 ID），零资金分录、零补偿；
/// 同处置重放幂等；别的处置 ID 不能再封闭同一 Sale。
#[tokio::test]
async fn postgres_consignment_recovery_cancel_unpaid_closes_the_sale_with_zero_funding() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let runtime = database.store().await.expect("运行账号");
    let pool = owner.postgres_pool();
    seed_refund(pool).await;
    seed_consumption(pool).await;
    succeeds(
        pool,
        &disposition_sql(
            DISPOSITION,
            ("close_at_limit", "held_unpaid_recovered", "manual_or_auto"),
            true,
        ),
    )
    .await;
    let contract = uuid(CONTRACT);
    let funding = "SELECT (SELECT count(*) FROM public.settlement_funding_entries)
                        + (SELECT count(*) FROM public.settlement_funding_allocations)
                        + (SELECT count(*) FROM public.settlement_funding_sources)
                        + (SELECT count(*) FROM public.compensation_ledger)";
    let before = count(pool, funding).await;

    assert!(runtime.test_cancel_unpaid("", contract, uuid(DISPOSITION)).await.expect("封闭"));
    let (kind, basis): (String, String) = sqlx::query_as(
        "SELECT closure_kind, basis_ref FROM public.consignment_sale_closures
         WHERE contract_id = $1",
    )
    .bind(contract)
    .fetch_one(pool)
    .await
    .expect("封闭事实");
    assert_eq!(
        (kind.as_str(), basis),
        ("cancelled_unpaid", format!("unpaid-recovery:{DISPOSITION}"))
    );
    assert_eq!(count(pool, funding).await, before, "零资金分录、零补偿");

    assert!(
        !runtime.test_cancel_unpaid("", contract, uuid(DISPOSITION)).await.expect("重放"),
        "同处置重放是幂等的"
    );
    assert!(
        runtime.test_cancel_unpaid("", contract, uuid(OTHER_DISPOSITION)).await.is_err(),
        "别的处置不能再封闭同一 Sale"
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_sale_closures").await, 1);
}

/// 已付款的 Sale 不能按未付取消：funded 投影与收款分录在，写端拒绝，没有封闭事实。
#[tokio::test]
async fn postgres_consignment_recovery_cancel_unpaid_refuses_a_paid_sale() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let runtime = database.store().await.expect("运行账号");
    let pool = owner.postgres_pool();
    seed_unpaid(pool).await;
    succeeds(pool, &refund_dispatch_sql()).await;
    seed_consumption(pool).await;
    succeeds(
        pool,
        &disposition_sql(
            DISPOSITION,
            ("close_at_limit", "held_unpaid_recovered", "manual_or_auto"),
            true,
        ),
    )
    .await;
    pay(pool).await;

    // 写端先于数据库的封闭守卫拒绝：确定的冲突，而不是存储故障。
    let refused = runtime.test_cancel_unpaid("", uuid(CONTRACT), uuid(DISPOSITION)).await;
    assert!(
        matches!(refused, Err(FundingError::Conflict)),
        "已付款的 Sale 不得按未付取消: {refused:?}"
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_sale_closures").await, 0);
}

/// 把成交待履约的挂单转为 held_off_market(unpaid_recovered) 的同事务 SQL：命令（带挂单变更的回执）、
/// 处置行、未付取消封闭、事件与挂单版本。`pieces` 决定哪些部分写入，用来造反例。
struct HoldBatch {
    enabled: bool,
    closure: bool,
    disposition: Option<(&'static str, &'static str, &'static str)>,
    reason: &'static str,
    event_actor: &'static str,
}

impl HoldBatch {
    const fn complete() -> Self {
        Self {
            enabled: true,
            closure: true,
            disposition: Some(("close_at_limit", "held_unpaid_recovered", "manual_or_auto")),
            reason: "unpaid_recovered",
            event_actor: "worker",
        }
    }

    fn sql(&self) -> String {
        let intake = intake().intake();
        let body = json!({"schema_version": 2, "operation": "resolve_return",
            "disposition": "disposition_recorded", "command_id": RESOLVE_COMMAND,
            "contract_id": CONTRACT, "contract_version": 1, "consumption_id": CONSUMPTION,
            "disposition_id": DISPOSITION,
            "listing_changes": [{"listing_id": LISTING, "listing_version": 7,
                "state": "held_off_market", "hold_reason": self.reason}]});
        let mut sql = command(
            RESOLVE_COMMAND,
            ("worker", "consignment"),
            "resolve_return",
            ("return_intake", Some(&intake), &intake),
            None,
            200,
            &body,
            V7_AT,
        );
        if let Some((buyer_right, effect, relist)) = self.disposition {
            sql += &disposition_sql(DISPOSITION, (buyer_right, effect, relist), self.enabled).replace(
                "(SELECT command_id FROM public.consignment_commands ORDER BY command_id LIMIT 1)",
                &format!("'{RESOLVE_COMMAND}'"),
            );
        }
        if self.closure {
            sql += &format!(
                "INSERT INTO public.consignment_sale_closures
                    (contract_id, closure_kind, basis_ref, closed_at)
                 VALUES ('{CONTRACT}', 'cancelled_unpaid', 'unpaid-recovery:{DISPOSITION}',
                         '{V7_AT}');"
            );
        }
        sql + &event(
            7,
            RESOLVE_COMMAND,
            (self.event_actor, "hold_set"),
            ("sale_pending", "held_off_market"),
            Some(self.reason),
            V7_AT,
        ) + &listing_update("held_off_market", Some(self.reason), 7, V7_AT)
    }
}

/// 未付收回处置之后的存量：挂单 `held_off_market/unpaid_recovered`（版本 7）、Sale 有 `cancelled_unpaid` 封闭、
/// 处置允许自动再售（冻结开关由参数决定）、position 仍是 `committed`（原件从未起写，原件腿没有 Send 尝试）。
/// 经数据库放行的真实写序（处置行、封闭事实、命令回执、事件与挂单版本同一事务）造出，不放宽任何约束。
pub(super) async fn seed_unpaid_recovered(pool: &PgPool, enabled: bool) {
    seed_refund(pool).await;
    seed_consumption(pool).await;
    // 服务开放且代次与挂单登记时一致：自动重列与手动重列的同一组前置条件。
    succeeds(
        pool,
        "UPDATE public.consignment_lookup_control SET admitting = true, enabled_epoch = 1 WHERE id = 1;",
    )
    .await;
    succeeds(pool, &HoldBatch { enabled, ..HoldBatch::complete() }.sql()).await;
}

/// 挂单边的数据库守卫：同事务的处置行（关闭原买方交割、挂单效果 held_unpaid_recovered）与
/// cancelled_unpaid 封闭事实齐备才放行；缺任一项、原因不是 unpaid_recovered、事件 actor 不是 worker 都拒绝。
#[tokio::test]
async fn postgres_consignment_recovery_sale_pending_hold_needs_the_disposition_and_the_closure() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let pool = owner.postgres_pool();
    seed_refund(pool).await;
    seed_consumption(pool).await;
    let guard = Some("consignment_listing_transition");

    let no_closure = HoldBatch { closure: false, ..HoldBatch::complete() };
    fails(pool, &no_closure.sql(), "23514", guard).await;
    let no_disposition = HoldBatch { disposition: None, ..HoldBatch::complete() };
    fails(pool, &no_disposition.sql(), "23514", guard).await;
    let unchanged = HoldBatch {
        disposition: Some(("close_at_limit", "unchanged", "forbidden")),
        ..HoldBatch::complete()
    };
    fails(pool, &unchanged.sql(), "23514", guard).await;
    let retained = HoldBatch {
        disposition: Some(("retain", "held_unpaid_recovered", "manual_or_auto")),
        ..HoldBatch::complete()
    };
    fails(pool, &retained.sql(), "23514", guard).await;
    let other_reason = HoldBatch { reason: "operations_hold", ..HoldBatch::complete() };
    // 事件边闭集先于挂单守卫拒绝：sale_pending 的 hold_set 原因只能是 unpaid_recovered。
    fails(pool, &other_reason.sql(), "23514", Some("consignment_event_edge")).await;
    let admin = HoldBatch { event_actor: "admin", ..HoldBatch::complete() };
    fails(pool, &admin.sql(), "23514", None).await;
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_listings WHERE state = 'sale_pending'"
        )
        .await,
        1,
        "反例都已回滚，挂单仍是成交待履约"
    );

    succeeds(pool, &HoldBatch::complete().sql()).await;
    let (state, reason): (String, Option<String>) =
        sqlx::query_as("SELECT state, reason_code FROM public.consignment_listings")
            .fetch_one(pool)
            .await
            .expect("读取挂单");
    assert_eq!((state.as_str(), reason.as_deref()), ("held_off_market", Some("unpaid_recovered")));
}

const FORCED_COMMAND: &str = "c7e00000-0000-5000-8000-0000000000d1";

/// 固定决定：关闭原买方交割（达上限）、挂单转 `held_unpaid_recovered`、允许手动或自动再售。
#[allow(clippy::unnecessary_wraps, reason = "签名必须与矩阵判定函数一致")]
fn forced_unpaid_recovery(
    _facts: &ReturnDispositionFacts,
) -> Result<ReturnDispositionDecision, RecoveryDomainError> {
    Ok(ReturnDispositionDecision {
        buyer_right: BuyerRightEffect::CloseAtLimit,
        refund: RefundEffect::None,
        principal: PrincipalEffect::None,
        listing: ListingEffect::HeldUnpaidRecovered,
        relist: RelistEligibility::ManualOrAuto,
    })
}

/// 本期不支持的挂单效果（转为已付退回下架）：效果前置不成立，必须在写入任何行之前确定拒绝。
#[allow(clippy::unnecessary_wraps, reason = "签名必须与矩阵判定函数一致")]
fn forced_paid_returned(
    _facts: &ReturnDispositionFacts,
) -> Result<ReturnDispositionDecision, RecoveryDomainError> {
    Ok(ReturnDispositionDecision {
        buyer_right: BuyerRightEffect::Retain,
        refund: RefundEffect::None,
        principal: PrincipalEffect::None,
        listing: ListingEffect::HeldPaidReturned,
        relist: RelistEligibility::ManualOnly,
    })
}

fn forced_command() -> ResolveReturn {
    ResolveReturn {
        command_id: CommandId::from_uuid(uuid(FORCED_COMMAND)),
        request_hash: RequestHash::new([9; 32]),
        consumption_id: uuid(CONSUMPTION),
        intake_id: uuid(&intake().intake()),
        auto_relist: AutoRelistSetting { enabled: true, config_digest: [9; 32] },
    }
}

/// resolve_return 的应用效果端到端：决定关闭原买方交割并转下架时，同一事务里写 cancel_unpaid 封闭事实、
/// 挂单 sale_pending -> held_off_market(unpaid_recovered)（版本、hold_set 事件、回执里的挂单变更）与订单阶段事件，
/// 零资金分录；重放不改库；随后自动重列（冻结开关为开）先回库再得到新代。不支持的挂单效果确定拒绝、不写任何行。
#[tokio::test]
async fn postgres_consignment_recovery_resolve_return_applies_the_unpaid_recovery_effects() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_refund(pool).await;
    seed_consumption(pool).await;
    succeeds(
        pool,
        "UPDATE public.consignment_lookup_control SET admitting = true, enabled_epoch = 1 WHERE id = 1;",
    )
    .await;
    let funding = "SELECT (SELECT count(*) FROM public.settlement_funding_entries)
                        + (SELECT count(*) FROM public.settlement_funding_allocations)
                        + (SELECT count(*) FROM public.settlement_funding_sources)
                        + (SELECT count(*) FROM public.compensation_ledger)";
    let stage_events = "SELECT count(*) FROM public.realtime_events
                        WHERE kind = 'consignment.order_stage_changed'";
    let (funding_before, stages_before) =
        (count(pool, funding).await, count(pool, stage_events).await);

    // 反例：不支持的挂单效果确定拒绝，没有任何行落库。
    let refused = store
        .test_resolve_return_with(forced_command(), forced_paid_returned)
        .await
        .expect_err("转已付退回下架不在本期效果内");
    assert_eq!(refused, ConsignmentError::Deposit(DepositBusinessRejection::EvidenceConflict));
    for sql in [
        "SELECT count(*) FROM public.consignment_return_dispositions",
        "SELECT count(*) FROM public.consignment_sale_closures",
    ] {
        assert_eq!(count(pool, sql).await, 0);
    }
    assert_eq!(listing_state(pool).await, ("sale_pending".to_owned(), None, 6));

    // 正例：效果同事务落库。
    let receipt = store
        .test_resolve_return_with(forced_command(), forced_unpaid_recovery)
        .await
        .expect("处置并应用效果");
    assert!(!receipt.replayed);
    assert_eq!(receipt.decision.buyer_right, BuyerRightEffect::CloseAtLimit);
    let basis: String = sqlx::query_scalar(
        "SELECT basis_ref FROM public.consignment_sale_closures
         WHERE contract_id = $1 AND closure_kind = 'cancelled_unpaid'",
    )
    .bind(uuid(CONTRACT))
    .fetch_one(pool)
    .await
    .expect("未付取消封闭事实");
    assert_eq!(basis, format!("unpaid-recovery:{}", receipt.disposition_id));
    assert_eq!(
        listing_state(pool).await,
        ("held_off_market".to_owned(), Some("unpaid_recovered".to_owned()), 7)
    );
    let (actor, kind): (String, String) = sqlx::query_as(
        "SELECT actor_kind, event_kind FROM public.consignment_events WHERE state_version = 7",
    )
    .fetch_one(pool)
    .await
    .expect("hold_set 事件");
    assert_eq!((actor.as_str(), kind.as_str()), ("worker", "hold_set"));
    let changes: serde_json::Value = sqlx::query_scalar(
        "SELECT result_body -> 'listing_changes' FROM public.consignment_commands
         WHERE command_id = $1",
    )
    .bind(uuid(FORCED_COMMAND))
    .fetch_one(pool)
    .await
    .expect("回执里的挂单变更");
    assert_eq!(changes[0]["hold_reason"], "unpaid_recovered");
    assert_eq!(count(pool, funding).await, funding_before, "零资金分录、零补偿");
    assert_eq!(count(pool, stage_events).await, stages_before + 1, "订单阶段事件同事务");
    let stage: serde_json::Value = sqlx::query_scalar(
        "SELECT public_payload FROM public.realtime_events
         WHERE kind = 'consignment.order_stage_changed' ORDER BY stream_sequence DESC LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .expect("订单阶段事件");
    assert_eq!(
        (&stage["order_state"], &stage["payment_stage"], &stage["payout_stage"]),
        (&json!("cancelled"), &json!("closed"), &json!("closed")),
        "未付取消的订单投影为已取消，付款腿与卖款腿封闭"
    );

    // 重放：同命令同请求返回原处置，不改库。
    let replay = store
        .test_resolve_return_with(forced_command(), forced_unpaid_recovery)
        .await
        .expect("重放");
    assert!(replay.replayed);
    assert_eq!(replay.disposition_id, receipt.disposition_id);
    assert_eq!(count(pool, stage_events).await, stages_before + 1);

    // 自动重列：冻结开关为开、挂单已是未付收回下架、Sale 已封闭、原件从未离库 -> 先回库再得到新代。
    let port: Arc<dyn AutoRelistStore> = store;
    let relist = AutoRelistService::new(port)
        .relist(
            PendingAutoRelist { disposition_id: receipt.disposition_id, listing_id: uuid(LISTING) },
            5,
        )
        .await
        .expect("自动重列");
    assert!(matches!(relist.outcome, AutoRelistOutcome::Relisted { generation: 2, .. }));
    assert_eq!(listing_state(pool).await, ("available".to_owned(), None, 8));
}

async fn listing_state(pool: &PgPool) -> (String, Option<String>, i64) {
    sqlx::query_as("SELECT state, reason_code, state_version FROM public.consignment_listings")
        .fetch_one(pool)
        .await
        .expect("读取挂单")
}
