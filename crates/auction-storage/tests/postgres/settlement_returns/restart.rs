//! 结算退件 Take 不经领取证明的机器复核出口（PostgreSQL 18，运行账号，经真实端口推进）：
//! - 确定未外写（准备期即被拒绝的终态 failed，结果记录未开始写出且非未知）：案件 owner 在收口时以
//!   新 fence 重新排队同一 Take、案件留在 Take 待执行，再执行成功后照常领回入账；每案最多重启两次，
//!   用满后转结果未知；开始写出或结果未知的失败不重启；
//! - 激活前冲突：同一邮件另一 owner 的意图尚在进行或曾进入发送围栏时保持结果未知；该意图以持久
//!   事实确定从未执行后案件交接回 Take 待执行并照常激活，此后投递台不能再重试复活那条意图；
//! - 重启与交接共用同一准入：`write_started` 与 `outcome_unknown` 都必须是明确的 JSON 布尔 `false`，
//!   缺键、null 或错误类型（如字符串 `"false"`）一律按可能已执行处理，不重启、不交接。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::assert_matches;
use std::time::Duration as StdDuration;

use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminPort, DeliveryClaimExclusions,
    DeliverySendResult, DeliveryTakeExpectation, DeliveryTransportError, DeliveryWorkClaim,
    DeliveryWorkStore,
};
use auction_application::settlement::{SettlementReturnAdminPort, SettlementReturnWorkStore};
use auction_domain::{
    SettlementReturnAction, SettlementReturnCaseStatus, SettlementReturnTakeUnknownReason,
};
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::flows::{PAYOUT, Returned, handle, returned_payout, seed_spent_payout};
use super::sweep::{MailSession, seed_mail_session, take_into_sending};
use crate::support;

/// 现金退件经 Admin 领回代管、Take 激活为待执行投递；返回案件与 Take 投递。
async fn activated_take(store: &DatabaseStore, sequence: i64, mid: &str) -> (Returned, Uuid) {
    let pool = store.postgres_pool();
    let returned = returned_payout(store, sequence, mid).await;
    seed_spent_payout(pool, &returned).await;
    assert_eq!(
        handle(store, returned.case, 1, SettlementReturnAction::ReclaimAndHold).await,
        SettlementReturnCaseStatus::TakePending
    );
    store
        .activate_due_return_take(Timestamp::now(), super::take_worlds())
        .await
        .expect("激活")
        .expect("必须激活");
    let take = take_delivery(pool, returned.case).await.expect("激活后有 Take 投递");
    (returned, take)
}

async fn take_delivery(pool: &PgPool, case: Uuid) -> Option<Uuid> {
    sqlx::query_scalar(
        "SELECT delivery_id FROM public.settlement_return_actions WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取 Take 投递")
}

/// 投递 Worker 领取 Take 投递（准备期，尚未进入发送围栏）。
async fn claim_take(store: &DatabaseStore, take: Uuid) -> DeliveryWorkClaim {
    let now = Timestamp::now();
    let grace = StdDuration::from_secs(60);
    DeliveryWorkStore::begin_claim_cycle(store, now, grace).await.expect("领取前清理");
    DeliveryWorkStore::claim_due(
        store,
        "returns-restart-delivery".to_owned(),
        now,
        now + Duration::minutes(2),
        grace,
        DeliveryClaimExclusions::default(),
        10,
    )
    .await
    .expect("领取投递")
    .into_iter()
    .find(|claim| claim.delivery_id == take)
    .expect("领取到 Take 投递")
}

/// 准备期确定的本地拒绝：投递终态 failed，结果记录未开始写出且不是未知。
async fn reject_before_write(store: &DatabaseStore, take: Uuid) {
    let claim = claim_take(store, take).await;
    store.reject_preparation(claim, Timestamp::now()).await.expect("准备期拒绝收口");
}

async fn case_state(pool: &PgPool, case: Uuid) -> (String, String, String, i64) {
    sqlx::query_as(
        "SELECT c.status, a.execution_state, d.status, d.fencing_version
         FROM public.settlement_return_cases c
         JOIN public.settlement_return_actions a ON a.return_mail_id = c.return_mail_id
         JOIN public.deliveries d ON d.id = a.delivery_id
         WHERE c.return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取案件与 Take")
}

async fn last_event(pool: &PgPool, case: Uuid) -> (String, Value) {
    sqlx::query_as(
        "SELECT event_kind, details FROM public.settlement_return_events
         WHERE return_mail_id = $1 ORDER BY case_version DESC LIMIT 1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取最新事件")
}

fn take_expectation(session: &MailSession, mid: &str) -> DeliveryTakeExpectation {
    DeliveryTakeExpectation {
        world_id: "main".to_owned(),
        external_mail_id: mid.to_owned(),
        credential: Some(session.binding()),
    }
}

/// 确定未外写：收口时以新 fence 重新排队同一 Take，案件留在 Take 待执行；再执行成功后照常领回、
/// 返还一次并按计划代管。
#[tokio::test]
async fn postgres_return_take_not_started_restarts_and_settles() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let (returned, take) = activated_take(&store, 7_660_001, "998101").await;
    let (_, _, _, fence_before) = case_state(pool, returned.case).await;
    reject_before_write(&store, take).await;

    let progress =
        store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须推进");
    assert_eq!(
        (progress.status, progress.event_kind),
        (SettlementReturnCaseStatus::TakePending, "take_activated"),
        "确定未外写不进入结果未知"
    );
    let (status, action, delivery, fence) = case_state(pool, returned.case).await;
    assert_eq!(
        (status.as_str(), action.as_str(), delivery.as_str()),
        ("take_pending", "enqueued", "pending")
    );
    assert!(fence > fence_before, "重启使用新 fence");
    let restart_clock: bool = sqlx::query_scalar(
        "SELECT ((result_metadata->>'not_started_restarted_at')::timestamptz = updated_at) IS TRUE
         FROM public.deliveries WHERE id = $1",
    )
    .bind(take)
    .fetch_one(pool)
    .await
    .expect("读取未发出 Take 的重启时间");
    assert!(restart_clock, "重启记录与更新必须共用一个时刻");
    let (kind, details) = last_event(pool, returned.case).await;
    assert_eq!(kind, "take_activated");
    assert_eq!(
        (details["reason"].clone(), details["restarts"].clone()),
        (json!("take_not_started"), json!(1))
    );

    let claim = take_into_sending(&store, take, &take_expectation(&session, "998101")).await;
    store
        .succeed(
            claim,
            DeliverySendResult {
                external_reference: "mooglemail:main:take:998101".to_owned(),
                result_metadata: json!({
                    "operation": "take", "external_mail_id": "998101",
                    "proof": "action_removed_or_success_page",
                    "expected_credits": PAYOUT, "expected_cod": 0, "expected_attachment_ids": [],
                }),
            },
            Timestamp::now(),
        )
        .await
        .expect("Take 成功");
    let settled =
        store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须收口");
    assert_eq!(settled.status, SettlementReturnCaseStatus::Held);
    let returns: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_funding_returns WHERE return_mail_id = $1",
    )
    .bind(returned.case)
    .fetch_one(pool)
    .await
    .expect("读取返还");
    assert_eq!(returns, 1, "资金返还一次");

    pool.close().await;
}

/// 每案最多重启两次，第三次确定未外写转结果未知；开始写出（`write_started=true`）的失败从不重启。
#[tokio::test]
async fn postgres_return_take_restart_is_bounded_and_needs_unstarted_proof() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let (returned, take) = activated_take(&store, 7_660_011, "998111").await;
    for restart in 1..=2 {
        reject_before_write(&store, take).await;
        let progress =
            store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须推进");
        assert_eq!(progress.status, SettlementReturnCaseStatus::TakePending, "第 {restart} 次");
    }
    reject_before_write(&store, take).await;
    let progress =
        store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须推进");
    assert_eq!(progress.status, SettlementReturnCaseStatus::TakeUnknown, "用满两次后转结果未知");
    let failure: Option<String> = sqlx::query_scalar(
        "SELECT failure_code FROM public.settlement_return_actions WHERE return_mail_id = $1",
    )
    .bind(returned.case)
    .fetch_one(pool)
    .await
    .expect("读取动作");
    assert_eq!(failure.as_deref(), Some("settlement_return_take_not_started"));

    // 另一案件：失败但记录已开始写出，不能证明未外写，直接转结果未知。
    let (other, other_take) = activated_take(&store, 7_660_021, "998121").await;
    let claim = take_into_sending(&store, other_take, &take_expectation(&session, "998121")).await;
    DeliveryWorkStore::fail(
        &store,
        claim,
        DeliveryTransportError {
            failure_code: "remote_rejected".to_owned(),
            safe_detail: "远端拒绝".to_owned(),
            outcome_unknown: false,
            external_reference: None,
            result_metadata: json!({"operation": "take", "write_started": true}),
        },
        Timestamp::now(),
    )
    .await
    .expect("失败收口");
    let progress =
        store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须推进");
    assert_eq!(
        (progress.return_mail_id, progress.status),
        (other.case, SettlementReturnCaseStatus::TakeUnknown)
    );
    assert_eq!(case_state(pool, other.case).await.2, "failed", "不重启开始写出的 Take");

    pool.close().await;
}

/// 写入同一邮件的一条入站领取意图（另一 owner），初始为在途。
async fn insert_intake_take(pool: &PgPool, mid: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count)
         VALUES ($1, $2, decode(repeat('6c', 32), 'hex'), 'mail_intake', $3, 'SellerPlayer',
                 jsonb_build_object('schema_version', 1, 'operation', 'take', 'world', 'main',
                     'external_mail_id', $4::text),
                 'pending', 1, 0)",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().to_string())
    .bind(mid)
    .execute(pool)
    .await
    .expect("写入同邮件的入站领取意图");
    id
}

/// 激活前冲突：另一 owner 的意图在途或曾进入发送围栏时保持结果未知；以持久事实确定从未执行后
/// 交接回 Take 待执行并照常激活，之后投递台不能重试复活那条意图。
#[tokio::test]
async fn postgres_return_take_conflict_hands_over_after_unexecuted_failure() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let returned = returned_payout(&store, 7_660_031, "998131").await;
    seed_spent_payout(pool, &returned).await;
    assert_eq!(
        handle(&store, returned.case, 1, SettlementReturnAction::ReclaimAndHold).await,
        SettlementReturnCaseStatus::TakePending
    );
    // 领回命令提交之后、激活之前出现的同邮件意图（命令时已复核互斥）。
    let intake = insert_intake_take(pool, "998131").await;
    let progress = store
        .activate_due_return_take(Timestamp::now(), super::take_worlds())
        .await
        .expect("激活")
        .expect("推进");
    assert_eq!(progress.status, SettlementReturnCaseStatus::TakeUnknown, "在途意图阻断激活");
    assert_eq!(
        store.settlement_return_detail(returned.case).await.expect("详情").take_unknown_reason,
        Some(SettlementReturnTakeUnknownReason::DispositionConflict),
        "没有 Take 投递的结果未知说明为同邮件冲突"
    );
    assert_eq!(take_delivery(pool, returned.case).await, None, "冲突时不写入投递");
    assert!(
        store
            .activate_due_return_take(Timestamp::now(), super::take_worlds())
            .await
            .expect("激活")
            .is_none(),
        "仍被阻断时不推进"
    );

    // 失败但曾进入发送围栏：不能证明从未执行，仍保持结果未知。
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'failed', failure_code = 'remote_rejected', sending_at = now(),
             completed_at = now(), claimed_at = now(), attempt_count = 1, updated_at = now(),
             result_metadata = '{\"write_started\": false, \"outcome_unknown\": false}'::jsonb
         WHERE id = $1",
    )
    .bind(intake)
    .execute(pool)
    .await
    .expect("意图失败（已进入发送围栏）");
    assert!(
        store
            .activate_due_return_take(Timestamp::now(), super::take_worlds())
            .await
            .expect("激活")
            .is_none()
    );

    // 以持久事实确定从未执行（与投递 Worker 准备期拒绝写入的形状一致）：从未进入发送围栏、
    // 没有尝试、记录未开始写出且非未知。
    sqlx::query(
        "UPDATE public.deliveries SET sending_at = NULL, failure_code = 'invalid_payload',
             updated_at = now(),
             result_metadata = '{\"stage\": \"preflight_blocked\", \"write_started\": false,
                                 \"outcome_unknown\": false,
                                 \"failure_code\": \"invalid_payload\"}'::jsonb
         WHERE id = $1",
    )
    .bind(intake)
    .execute(pool)
    .await
    .expect("意图确定从未执行");
    let progress = store
        .activate_due_return_take(Timestamp::now(), super::take_worlds())
        .await
        .expect("交接")
        .expect("推进");
    assert_eq!(
        (progress.status, progress.event_kind),
        (SettlementReturnCaseStatus::TakePending, "take_requested")
    );
    assert_eq!(last_event(pool, returned.case).await.1["handover"], json!(true));
    let progress = store
        .activate_due_return_take(Timestamp::now(), super::take_worlds())
        .await
        .expect("激活")
        .expect("推进");
    assert_eq!(progress.event_kind, "take_activated");
    assert!(take_delivery(pool, returned.case).await.is_some(), "交接后照常写入 Take 投递");

    // 普通重试不能复活已交接的意图。
    let fence: i64 =
        sqlx::query_scalar("SELECT fencing_version FROM public.deliveries WHERE id = $1")
            .bind(intake)
            .fetch_one(pool)
            .await
            .expect("读取意图 fence");
    let action = DeliveryAdminAction::RetryFailed;
    let retried = DeliveryAdminPort::execute(
        &store,
        DeliveryAdminCommand {
            command_id: Uuid::new_v4(),
            delivery_id: intake,
            expected_fencing_version: fence,
            action,
            request_hash: action.request_hash(intake, fence, None, None, "admin-returns"),
            external_reference: None,
            failure_code: None,
            actor_id: "admin-returns".to_owned(),
            request_id: None,
        },
    )
    .await;
    assert_matches!(
        retried,
        Err(auction_application::core::ApplicationError::DeliveryAdmin(
            auction_application::delivery::DeliveryAdminError::Invalid
        )),
        "已交接的意图不能被重试复活：{retried:?}"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(intake)
        .fetch_one(pool)
        .await
        .expect("读取意图");
    assert_eq!(status, "failed");

    pool.close().await;
}

/// 结果元数据不能证明未执行的形状：缺 `outcome_unknown`、为 null、为字符串，或 `write_started`
/// 为字符串。其余字段与投递 Worker 准备期拒绝写入的形状一致。
fn unproven_unexecuted_metadata() -> [Value; 4] {
    [
        json!({"stage": "preflight_blocked", "write_started": false,
               "failure_code": "invalid_payload"}),
        json!({"stage": "preflight_blocked", "write_started": false, "outcome_unknown": null,
               "failure_code": "invalid_payload"}),
        json!({"stage": "preflight_blocked", "write_started": false, "outcome_unknown": "false",
               "failure_code": "invalid_payload"}),
        json!({"stage": "preflight_blocked", "write_started": "false", "outcome_unknown": false,
               "failure_code": "invalid_payload"}),
    ]
}

/// 改写投递的结果元数据（只改元数据，状态与 fence 不变）。
async fn overwrite_metadata(pool: &PgPool, delivery: Uuid, metadata: &Value) {
    sqlx::query("UPDATE public.deliveries SET result_metadata = $2 WHERE id = $1")
        .bind(delivery)
        .bind(metadata)
        .execute(pool)
        .await
        .expect("改写结果元数据");
}

/// 案件 owner 的重启只认明确的布尔 `false`：准备期拒绝之后结果元数据缺键、为 null 或类型错误时，
/// 收口转结果未知，不以新 fence 重新排队（投递保持 failed、fence 不变）。
#[tokio::test]
async fn postgres_return_take_restart_requires_explicit_boolean_flags() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    for (index, metadata) in unproven_unexecuted_metadata().iter().enumerate() {
        let offset = i64::try_from(index).expect("用例序号");
        let mid = format!("{}", 998_141 + offset);
        let (returned, take) = activated_take(&store, 7_660_041 + offset, &mid).await;
        reject_before_write(&store, take).await;
        overwrite_metadata(pool, take, metadata).await;
        let (_, _, _, fence_before) = case_state(pool, returned.case).await;
        let progress =
            store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须推进");
        assert_eq!(
            (progress.return_mail_id, progress.status),
            (returned.case, SettlementReturnCaseStatus::TakeUnknown),
            "{metadata}"
        );
        let (_, _, delivery, fence) = case_state(pool, returned.case).await;
        assert_eq!((delivery.as_str(), fence), ("failed", fence_before), "{metadata}：不重启");
    }

    pool.close().await;
}

/// 交接同样只认明确的布尔 `false`：另一 owner 的失败意图从未进入发送围栏，但结果元数据缺键、
/// 为 null 或类型错误时，不能证明从未执行，案件保持结果未知、不交接、不写入 Take 投递。
#[tokio::test]
async fn postgres_return_take_handover_requires_explicit_boolean_flags() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    for (index, metadata) in unproven_unexecuted_metadata().iter().enumerate() {
        let offset = i64::try_from(index).expect("用例序号");
        let mid = format!("{}", 998_151 + offset);
        let returned = returned_payout(&store, 7_660_051 + offset, &mid).await;
        seed_spent_payout(pool, &returned).await;
        assert_eq!(
            handle(&store, returned.case, 1, SettlementReturnAction::ReclaimAndHold).await,
            SettlementReturnCaseStatus::TakePending
        );
        let intake = insert_intake_take(pool, &mid).await;
        let progress = store
            .activate_due_return_take(Timestamp::now(), super::take_worlds())
            .await
            .expect("激活")
            .expect("推进");
        assert_eq!(progress.status, SettlementReturnCaseStatus::TakeUnknown, "在途意图阻断激活");
        // 意图终态失败、从未进入发送围栏，但结果元数据不能证明从未执行。
        sqlx::query(
            "UPDATE public.deliveries
             SET status = 'failed', failure_code = 'invalid_payload', completed_at = now(),
                 claimed_at = now(), attempt_count = 1, updated_at = now(), result_metadata = $2
             WHERE id = $1",
        )
        .bind(intake)
        .bind(metadata)
        .execute(pool)
        .await
        .expect("意图失败");
        assert!(
            store
                .activate_due_return_take(Timestamp::now(), super::take_worlds())
                .await
                .expect("激活")
                .is_none(),
            "{metadata}：不能证明从未执行时不交接"
        );
        assert_eq!(take_delivery(pool, returned.case).await, None, "{metadata}");
        let status: String = sqlx::query_scalar(
            "SELECT status FROM public.settlement_return_cases WHERE return_mail_id = $1",
        )
        .bind(returned.case)
        .fetch_one(pool)
        .await
        .expect("读取案件");
        assert_eq!(status, "take_unknown", "{metadata}");
    }

    pool.close().await;
}
