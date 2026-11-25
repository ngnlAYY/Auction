//! 订单复核工作（PIWS4，方案 A）的 PG18 测试：领取互斥、待核对项快照、按租约完成与“绝不产生起写或外写许可”。
//!
//! 只测核心规则：并发与原子性（并发领取只一个拿到）、命令幂等（同租约重复完成得原结果、旧 token 拒绝）、
//! 资产外写（领取与完成不新增任何起写、尝试、命令或投递租约变化）、安全边界（`bound` 不能绕过证据写入，
//! 数据库守卫拒绝绕过运行账号代码的直接改写）。
//! 放在派发测试目录下，以复用“首派已起写”的夹具。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, OrderRecheckClaim, OrderRecheckLease, OrderRecheckOutcome,
    OrderRecheckWorkStore, RecheckOrderInput, RecordEvidenceInput,
};
use auction_domain::consignment::dispatch::EvidenceCategory;
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::CONTRACT;
use super::super::deposit::support::fails;
use super::super::order_admin::{ADMIN, command, service};
use super::super::returns::fixture::{
    Rts, SIGNED_VALIDATOR, consume_input, seed_original_started, service as return_service,
};
use super::fixture::{Seeded, at};
use crate::support::TestDatabase;

fn contract() -> Uuid {
    Uuid::parse_str(CONTRACT).expect("合同")
}

/// 订单事件流头部即当前订单版本（与 order_admin 测试同口径）。
async fn order_version(pool: &PgPool) -> u64 {
    let head: Option<i64> = sqlx::query_scalar(
        "SELECT next_sequence - 1 FROM public.realtime_streams WHERE stream_key = $1",
    )
    .bind(format!("consignment_order:{CONTRACT}"))
    .fetch_optional(pool)
    .await
    .expect("读取事件流");
    head.map_or(1, |head| head.max(1)).cast_unsigned()
}

/// Admin 排入一条复核工作（运行账号），返回工作 ID。
async fn enqueue(store: &DatabaseStore, pool: &PgPool, n: u128) -> Uuid {
    service(store.clone())
        .recheck(RecheckOrderInput {
            admin_actor: ADMIN.to_owned(),
            command_id: command(0x5400 + n),
            contract_id: contract(),
            expected_version: order_version(pool).await,
            request_id: None,
        })
        .await
        .expect("排入复核")
        .work_id
}

async fn claim_for(store: &DatabaseStore, lease: Duration) -> Option<OrderRecheckClaim> {
    let now = Timestamp::now();
    store
        .claim_due_order_recheck("recheck-worker".to_owned(), now, now + lease)
        .await
        .expect("领取")
}

/// 外写相关事实的快照：起写、派发尝试、Take 起写与 attempt、账本命令数，以及全部投递的状态 / fence / token。
async fn write_facts(pool: &PgPool) -> (i64, i64, i64, i64, i64, String) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.delivery_send_attempts),
                (SELECT count(*) FROM public.consignment_dispatch_attempts),
                (SELECT count(*) FROM public.mail_disposition_write_starts),
                (SELECT count(*) FROM public.mail_disposition_attempts),
                (SELECT count(*) FROM public.consignment_commands),
                (SELECT COALESCE(string_agg(id::text || status || fencing_version::text
                                            || COALESCE(lease_token::text, '-'), ',' ORDER BY id), '')
                 FROM public.deliveries)",
    )
    .fetch_one(pool)
    .await
    .expect("读取外写事实")
}

/// 未知派发：并发领取只一个拿到，快照列出该起写尝试；完成为 `still_unknown`，同租约重放得原结果，
/// 已完成的工作不再可领；`bound` 不能在证据缺席时写入（数据库守卫）；全程不新增任何起写、尝试或命令。
#[tokio::test]
async fn postgres_consignment_order_recheck_claims_once_and_lists_the_unknown_dispatch() {
    let (seeded, started) = Seeded::first_dispatch_started().await;
    let pool = seeded.pool();
    let runtime = Arc::new(seeded.runtime.clone());
    let work_id = enqueue(&runtime, pool, 1).await;
    let before = write_facts(pool).await;

    let (left, right) = tokio::join!(
        claim_for(&runtime, Duration::minutes(5)),
        claim_for(&runtime, Duration::minutes(5))
    );
    let claims: Vec<OrderRecheckClaim> = [left, right].into_iter().flatten().collect();
    assert_eq!(claims.len(), 1, "并发领取只能有一个拿到");
    let claim = &claims[0];
    assert_eq!(
        (claim.lease.work_id, claim.lease.fence, claim.contract_id),
        (work_id, 1, contract())
    );
    assert!(claim.return_takes.is_empty());
    let [attempt] = claim.dispatch_attempts.as_slice() else {
        panic!("应只列出一条未知派发：{:?}", claim.dispatch_attempts);
    };
    assert_eq!(attempt.attempt_id, started.attempt_receipt.attempt_id);
    assert_eq!((attempt.delivery_id, attempt.fence), (seeded.delivery_id, 1));
    assert_eq!(claim_for(&runtime, Duration::minutes(5)).await, None, "租约有效期内不可再领");

    // 安全边界：快照里的尝试没有 Sent 绑定时，连维护账号也写不进 bound。
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_order_recheck_results
                (work_id, result, claim_fence, completed_at)
             VALUES ('{work_id}', 'bound', 1, clock_timestamp())"
        ),
        "23514",
        Some("c7_recheck_result_bound"),
    )
    .await;

    let outcome = runtime.complete_order_recheck(claim.lease).await.expect("完成");
    assert_eq!(outcome, OrderRecheckOutcome::StillUnknown);
    let again = runtime.complete_order_recheck(claim.lease).await.expect("同租约重放");
    assert_eq!(again, OrderRecheckOutcome::StillUnknown);
    let forged = OrderRecheckLease { claim_token: Uuid::new_v4(), ..claim.lease };
    assert_eq!(
        runtime.complete_order_recheck(forged).await,
        Err(ConsignmentError::LeaseLost),
        "非当前 token 不得完成或改写结果"
    );
    assert_eq!(claim_for(&runtime, Duration::minutes(5)).await, None, "已完成的工作不可再领");
    let results: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.consignment_order_recheck_results")
            .fetch_one(pool)
            .await
            .expect("结果数");
    assert_eq!(results, 1);
    assert_eq!(write_facts(pool).await, before, "复核不得产生任何起写、尝试、命令或投递租约变化");
}

/// 正例：领取之后经既有写口补记 Sent 绑定，完成为 `bound`；反例：之后再排的复核领取时已无待核对项，
/// 完成为 `nothing_to_check`。过租后重新领取推进 fence，旧租约完成被拒。
#[tokio::test]
async fn postgres_consignment_order_recheck_outcome_follows_the_evidence_and_rejects_old_leases() {
    let (seeded, started) = Seeded::first_dispatch_started().await;
    let pool = seeded.pool();
    let runtime = Arc::new(seeded.runtime.clone());
    enqueue(&runtime, pool, 1).await;

    let expired = claim_for(&runtime, Duration::milliseconds(1)).await.expect("短租约领取");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let claim = claim_for(&runtime, Duration::minutes(5)).await.expect("过租后重新领取");
    assert_eq!(claim.lease.fence, expired.lease.fence + 1);
    assert_eq!(
        runtime.complete_order_recheck(expired.lease).await,
        Err(ConsignmentError::LeaseLost),
        "旧 token / 旧 fence 不得完成"
    );

    seeded
        .service()
        .record_evidence(RecordEvidenceInput {
            attempt: started.attempt,
            category: EvidenceCategory::SentBinding,
            remote_observation_id: "9001".to_owned(),
            observed_at: at("2026-01-01T05:00:00Z"),
            validator_version: "fake-validator-v1".to_owned(),
            evidence_digest: [0x54; 32],
        })
        .await
        .expect("经既有写口补记 Sent 绑定");
    assert_eq!(
        runtime.complete_order_recheck(claim.lease).await.expect("完成"),
        OrderRecheckOutcome::Bound
    );

    enqueue(&runtime, pool, 2).await;
    let empty = claim_for(&runtime, Duration::minutes(5)).await.expect("领取第二条");
    assert!(empty.dispatch_attempts.is_empty() && empty.return_takes.is_empty());
    assert_eq!(
        runtime.complete_order_recheck(empty.lease).await.expect("完成"),
        OrderRecheckOutcome::NothingToCheck
    );
}

/// 已起写未证的 Take：快照列出原 intake、工作、attempt 与 fence（原件腿派发已有 Sent 绑定，不列入）；
/// 无证明时完成为 `still_unknown`，且不新增任何起写或 attempt。
#[tokio::test]
async fn postgres_consignment_order_recheck_lists_the_started_unproven_take() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let pool = owner.postgres_pool();
    let runtime = Arc::new(database.store().await.expect("运行账号"));
    let (receipt, claim_fixture) = seed_original_started(pool, &runtime, Rts { n: 1 }).await;
    let work_id = enqueue(&runtime, pool, 1).await;
    let before = write_facts(pool).await;

    let claim = claim_for(&runtime, Duration::minutes(5)).await.expect("领取");
    assert_eq!(claim.lease.work_id, work_id);
    assert!(claim.dispatch_attempts.is_empty(), "已有 Sent 绑定的派发不是待核对项");
    let [take] = claim.return_takes.as_slice() else {
        panic!("应只列出一条未证 Take：{:?}", claim.return_takes);
    };
    assert_eq!((take.intake_id, take.work_id), (receipt.intake_id, receipt.work_id));
    assert_eq!((take.attempt_id, take.fence), (claim_fixture.attempt, claim_fixture.fence));

    assert_eq!(
        runtime.complete_order_recheck(claim.lease).await.expect("完成"),
        OrderRecheckOutcome::StillUnknown
    );
    assert_eq!(write_facts(pool).await, before, "复核不得产生任何起写或 attempt");
}

/// 数据库最后一道门（维护账号直接写，绕过运行账号代码）：身份列不可改；领取必须换新 token 且 fence 恰好加一；
/// 结果只能由当前有效租约写入；已有结果的工作过租后也不得再领取。
#[tokio::test]
async fn postgres_consignment_order_recheck_guards_reject_direct_lease_and_result_writes() {
    let (seeded, _started) = Seeded::first_dispatch_started().await;
    let pool = seeded.pool();
    let runtime = Arc::new(seeded.runtime.clone());
    let work_id = enqueue(&runtime, pool, 1).await;
    let result_sql = |fence: u64| {
        format!(
            "INSERT INTO public.consignment_order_recheck_results
                (work_id, result, claim_fence, completed_at)
             VALUES ('{work_id}', 'still_unknown', {fence}, clock_timestamp())"
        )
    };

    // (d) 过期租约写结果被拒。
    let expired = claim_for(&runtime, Duration::milliseconds(1)).await.expect("短租约领取");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    fails(pool, &result_sql(expired.lease.fence), "23514", Some("c7_recheck_result_lease")).await;

    let claim = claim_for(&runtime, Duration::seconds(2)).await.expect("过租后重新领取");
    // (d) 旧 fence 写结果被拒。
    fails(pool, &result_sql(expired.lease.fence), "23514", Some("c7_recheck_result_lease")).await;
    // (a) 身份列不可改。
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_order_rechecks SET requested_by = 'fake-other-admin'
             WHERE work_id = '{work_id}'"
        ),
        "23514",
        Some("c7_recheck_immutable"),
    )
    .await;
    // (b) fence 加二，或 fence 加一但不换 token。
    for set in [
        "claim_fence = claim_fence + 2, claim_token = gen_random_uuid()",
        "claim_fence = claim_fence + 1",
    ] {
        fails(
            pool,
            &format!(
                "UPDATE public.consignment_order_rechecks SET {set} WHERE work_id = '{work_id}'"
            ),
            "23514",
            Some("c7_recheck_fence"),
        )
        .await;
    }

    assert_eq!(
        runtime.complete_order_recheck(claim.lease).await.expect("完成"),
        OrderRecheckOutcome::StillUnknown
    );
    // (c) 已完成的工作在租约过期后被直接改写领取：形状合法（新 token、fence 加一、旧租约已过）仍被拒。
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_order_rechecks
             SET claim_token = gen_random_uuid(), claim_fence = claim_fence + 1,
                 lease_expires_at = clock_timestamp() + interval '5 minutes'
             WHERE work_id = '{work_id}'"
        ),
        "23514",
        Some("c7_recheck_finished"),
    )
    .await;
}

/// (e) Take 分支：快照含未证 Take 时直接写 `bound` 被守卫拒绝；经既有写口消费（精确 Take 证明）之后，
/// 同一租约的完成推导为 `bound`，并通过同一守卫。
#[tokio::test]
async fn postgres_consignment_order_recheck_take_is_bound_only_after_a_conclusive_proof() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let pool = owner.postgres_pool();
    let runtime = Arc::new(database.store().await.expect("运行账号"));
    let rts = Rts { n: 1 };
    let (receipt, claim_fixture) = seed_original_started(pool, &runtime, rts).await;
    let work_id = enqueue(&runtime, pool, 1).await;
    let claim = claim_for(&runtime, Duration::minutes(5)).await.expect("领取");
    assert_eq!(claim.return_takes.len(), 1);

    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_order_recheck_results
                (work_id, result, claim_fence, completed_at)
             VALUES ('{work_id}', 'bound', {fence}, clock_timestamp())",
            fence = claim.lease.fence
        ),
        "23514",
        Some("c7_recheck_result_bound"),
    )
    .await;

    return_service(&runtime)
        .consume(consume_input(&receipt, rts, claim_fixture, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("经既有写口消费，写入精确 Take 证明");
    assert_eq!(
        runtime.complete_order_recheck(claim.lease).await.expect("完成"),
        OrderRecheckOutcome::Bound
    );
}
