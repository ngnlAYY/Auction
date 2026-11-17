//! 结算退件 Take 结果未知后的原会话观察留证与 Admin 再次领回（PostgreSQL 18，运行账号）：
//! - 邮箱维护在原 Take 会话里的观察原因以事件 `take_observed` 只追加（同原因、同 fence 不重复），
//!   不改写投递、动作、资金或证据；换凭据版本的观察与巡检观察不到的原因都拒绝；
//! - 详情给出最近一次观察的具体原因，没有观察时由持久事实推出；告警按原因分作用域，原因改变后
//!   旧作用域由扫描关闭；
//! - 再次领回只在案件最近一次事件正是当前 Take fence 上的 `assets_claimable` 观察时允许：重新排队
//!   同一 Take 投递并追加投递审计，同命令重放返回原结果；新一轮执行结果未知后旧观察失效，须在
//!   新 fence 上再次观察；同邮件互斥仍生效；最终正常收口时领回事实与资金返还各一次；
//! - 观察之后原凭据换了版本：再次领回照常受理，但新一轮执行冻结的是执行当时的凭据身份，此后的
//!   复核只认这个新会话。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryFenceOutcome, DeliveryPreparedExpectation, DeliverySendCredentialBinding,
    DeliverySendResult, DeliveryTransportError, DeliveryWorkStore,
};
use auction_application::mail::MoogleMailboxEvidenceStore;
use auction_application::settlement::{
    SettlementReturnAdminPort, SettlementReturnCommand, SettlementReturnCommandResult,
    SettlementReturnError, SettlementReturnWorkStore,
};
use auction_domain::{
    CommandId, SettlementReturnAction, SettlementReturnBlocker, SettlementReturnCaseStatus,
    SettlementReturnTakeUnknownReason as Reason,
};
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::alerts::{enable_alerts, scan, signal};
use super::flows::PAYOUT;
use super::sweep::{
    UnknownTake, claim_take, facts, seed_mail_session, take_into_sending, unknown_take,
};
use crate::support;

/// 以巡检的形状记录一次原会话观察（自动重排已用满）。
async fn record(
    store: &DatabaseStore,
    take: &UnknownTake,
    fencing_version: i64,
    binding: &DeliverySendCredentialBinding,
    reason: Reason,
) -> Result<bool, ApplicationError> {
    store
        .record_return_take_observation(
            take.take_delivery,
            fencing_version,
            reason,
            2,
            &take.expectation,
            binding,
        )
        .await
}

async fn retry(
    store: &DatabaseStore,
    case: Uuid,
    version: i64,
    command_id: Uuid,
) -> Result<SettlementReturnCommandResult, ApplicationError> {
    let command = SettlementReturnCommand::new(
        CommandId::from_uuid(command_id),
        case,
        version,
        SettlementReturnAction::RetryTake,
        "admin-returns",
        Timestamp::now(),
    )
    .expect("构造命令");
    store.handle_settlement_return(command).await
}

async fn case_version(pool: &PgPool, case: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT case_version FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取案件版本")
}

async fn last_event(pool: &PgPool, case: Uuid) -> (String, Value, Option<String>) {
    sqlx::query_as(
        "SELECT event_kind, details, actor_id FROM public.settlement_return_events
         WHERE return_mail_id = $1 ORDER BY case_version DESC LIMIT 1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取最新事件")
}

async fn reason(store: &DatabaseStore, case: Uuid) -> Option<Reason> {
    store.settlement_return_detail(case).await.expect("案件详情").take_unknown_reason
}

fn rejected(
    result: Result<SettlementReturnCommandResult, ApplicationError>,
) -> SettlementReturnError {
    match result {
        Err(ApplicationError::SettlementReturn(error)) => error,
        other => panic!("应被类型化拒绝：{other:?}"),
    }
}

/// 以投递端口把一次 Take 执行收口为结果未知（已发出写入）。
async fn fail_unknown(
    store: &DatabaseStore,
    claim: auction_application::delivery::DeliveryWorkClaim,
) {
    DeliveryWorkStore::fail(
        store,
        claim,
        DeliveryTransportError {
            failure_code: "take_receipt_unproven".to_owned(),
            safe_detail: "Take 回执无法证明".to_owned(),
            outcome_unknown: true,
            external_reference: None,
            result_metadata: json!({"operation": "take", "write_started": true}),
        },
        Timestamp::now(),
    )
    .await
    .expect("结果未知收口为人工核对");
}

/// 原会话观察只追加事件、不改写事实；重复观察不再追加；换凭据版本与巡检观察不到的原因拒绝。
/// 详情与告警作用域随最近一次观察的原因改变，旧作用域由扫描关闭。
#[tokio::test]
async fn postgres_return_take_observation_records_reason_and_alert_scope() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let take = unknown_take(
        &store,
        &session,
        7_640_001,
        "996401",
        SettlementReturnAction::ReclaimAndHold,
        true,
    )
    .await;
    let case = take.returned.case;
    enable_alerts(&store).await;
    assert_eq!(reason(&store, case).await, Some(Reason::AwaitingObservation));
    scan(&store).await;
    assert_eq!(
        signal(pool, "settlement_return_take_unknown", "awaiting_observation").await,
        Some((true, 1))
    );
    assert_eq!(signal(pool, "settlement_return_quarantined", "auction").await, None);

    let before = facts(pool, &take).await;
    assert!(
        record(&store, &take, take.fencing_version, &session.binding(), Reason::MailMissing)
            .await
            .expect("观察留证必须确定提交"),
        "首次观察追加事件"
    );
    let after = facts(pool, &take).await;
    assert_eq!(after.case_version, before.case_version + 1);
    assert_eq!(
        (
            after.case_status.as_str(),
            after.action_state.as_str(),
            after.delivery_status.as_str(),
            after.takes,
            after.funding_returns,
            after.allocation_state.as_str(),
        ),
        (
            before.case_status.as_str(),
            before.action_state.as_str(),
            before.delivery_status.as_str(),
            before.takes,
            before.funding_returns,
            before.allocation_state.as_str(),
        ),
        "观察不改写案件状态、动作、投递或资金"
    );
    let (kind, details, actor) = last_event(pool, case).await;
    assert_eq!((kind.as_str(), actor), ("take_observed", None));
    assert_eq!(
        details,
        json!({
            "observation": "mail_missing",
            "delivery_id": take.take_delivery,
            "fencing_version": take.fencing_version,
            "sweep_retries": 2,
        })
    );
    assert!(
        !record(&store, &take, take.fencing_version, &session.binding(), Reason::MailMissing)
            .await
            .expect("重复观察不报错"),
        "同原因、同 fence 不重复追加"
    );
    assert_eq!(case_version(pool, case).await, after.case_version);
    assert_eq!(reason(&store, case).await, Some(Reason::MailMissing));
    scan(&store).await;
    assert_eq!(
        signal(pool, "settlement_return_take_unknown", "mail_missing").await,
        Some((true, 1))
    );
    assert_eq!(
        signal(pool, "settlement_return_take_unknown", "awaiting_observation")
            .await
            .map(|row| row.0),
        Some(false),
        "原因改变后旧作用域关闭"
    );

    // 换凭据版本的观察不是原 Take 会话；巡检观察不到的原因不能写成观察。都不写任何事实。
    let mut other_version = session.binding();
    other_version.credential_version = 2;
    assert!(
        record(&store, &take, take.fencing_version, &other_version, Reason::ReceiptUnproven)
            .await
            .is_err()
    );
    assert!(
        record(&store, &take, take.fencing_version, &session.binding(), Reason::SessionChanged)
            .await
            .is_err()
    );
    assert_eq!(case_version(pool, case).await, after.case_version);

    // 邮件整体缺失仍保持未知（失败关闭）：没有再次领回，阻断原因是缺少可领取的观察。
    let detail = store.settlement_return_detail(case).await.expect("案件详情");
    assert!(detail.allowed_actions.is_empty());
    assert!(detail.blockers.contains(&(
        SettlementReturnAction::RetryTake,
        SettlementReturnBlocker::ObservationRequired
    )));
    assert!(
        record(&store, &take, take.fencing_version, &session.binding(), Reason::ReceiptUnproven)
            .await
            .expect("原因改变时追加"),
    );
    assert_eq!(reason(&store, case).await, Some(Reason::ReceiptUnproven));

    pool.close().await;
}

/// 没有观察时原因由持久事实推出：原 Take 冻结的凭据换了版本即 `session_changed`。
#[tokio::test]
async fn postgres_take_unknown_reason_falls_back_to_persisted_facts() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let take = unknown_take(
        &store,
        &session,
        7_640_021,
        "996421",
        SettlementReturnAction::ReclaimAndHold,
        true,
    )
    .await;
    assert_eq!(reason(&store, take.returned.case).await, Some(Reason::AwaitingObservation));
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");
    sqlx::query("UPDATE public.credential_keys SET state_version = 2 WHERE id = $1")
        .bind(session.credential)
        .execute(maintenance.postgres_pool())
        .await
        .expect("原凭据换版本");
    maintenance.postgres_pool().close().await;
    assert_eq!(reason(&store, take.returned.case).await, Some(Reason::SessionChanged));
    // 已转结果未知以外的状态没有原因。
    let other = super::flows::returned_payout(&store, 7_640_022, "996422").await;
    assert_eq!(reason(&store, other.case).await, None);

    pool.close().await;
}

/// 再次领回只认当前 fence 上“完整仍在且可领取”的观察：没有观察、不可领取或陈旧版本都拒绝；
/// 成立时重新排队同一 Take 并留审计，重放返回原结果；新一轮执行结果未知后须重新观察；最终
/// 正常收口时领回事实与资金返还各一次。
#[tokio::test]
async fn postgres_return_take_retry_requires_claimable_observation_each_time() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let take = unknown_take(
        &store,
        &session,
        7_640_011,
        "996411",
        SettlementReturnAction::ReclaimAndHold,
        true,
    )
    .await;
    let case = take.returned.case;
    let unknown = facts(pool, &take).await;

    // 没有观察、或完整资产仍在却不可领取：缺少机器证明，拒绝且不改写任何事实。
    assert_eq!(
        rejected(retry(&store, case, unknown.case_version, Uuid::new_v4()).await),
        SettlementReturnError::ObservationRequired
    );
    assert!(
        record(&store, &take, take.fencing_version, &session.binding(), Reason::AssetsUnclaimable)
            .await
            .expect("留证")
    );
    let version = case_version(pool, case).await;
    assert_eq!(
        rejected(retry(&store, case, version, Uuid::new_v4()).await),
        SettlementReturnError::ObservationRequired
    );

    // 完整仍在且可领取：允许再次领回；陈旧版本仍被拒绝。
    assert!(
        record(&store, &take, take.fencing_version, &session.binding(), Reason::AssetsClaimable)
            .await
            .expect("留证")
    );
    let detail = store.settlement_return_detail(case).await.expect("案件详情");
    assert_eq!(detail.allowed_actions, [SettlementReturnAction::RetryTake]);
    assert_eq!(detail.take_unknown_reason, Some(Reason::AssetsClaimable));
    let version = detail.summary.case_version;
    assert_eq!(
        rejected(retry(&store, case, version - 1, Uuid::new_v4()).await),
        SettlementReturnError::Stale
    );
    let command_id = Uuid::new_v4();
    let result = retry(&store, case, version, command_id).await.expect("再次领回");
    assert_eq!(result.status, SettlementReturnCaseStatus::TakePending);
    let requeued = facts(pool, &take).await;
    assert_eq!(
        (
            requeued.case_status.as_str(),
            requeued.action_state.as_str(),
            requeued.delivery_status.as_str(),
            requeued.takes,
            requeued.funding_returns,
            requeued.allocation_state.as_str(),
        ),
        ("take_pending", "enqueued", "pending", 0, 0, "spent"),
        "再次领回只重新排队，不记领回、不动资金"
    );
    let (kind, details, actor) = last_event(pool, case).await;
    assert_eq!((kind.as_str(), actor.as_deref()), ("take_retry_requested", Some("admin-returns")));
    assert_eq!(details["fencing_version"], json!(take.fencing_version));
    assert_eq!(details["observed_case_version"], json!(version));
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records
         WHERE action = 'delivery.settlement_return_retake' AND entity_id = $1::text
           AND actor_type = 'admin' AND actor_id = 'admin-returns'",
    )
    .bind(take.take_delivery)
    .fetch_one(pool)
    .await
    .expect("读取投递审计");
    assert_eq!(audits, 1, "重新排队追加一条投递审计");
    let replay = retry(&store, case, version, command_id).await.expect("同命令重放");
    assert_eq!(replay, result, "同命令重放返回原结果");
    assert_eq!(facts(pool, &take).await, requeued, "重放不改写任何事实");

    // 新一轮执行（新 fence）再次结果未知：旧 fence 上的观察失效，须重新观察。
    let claim = take_into_sending(&store, take.take_delivery, &take.expectation).await;
    let second_fence = claim.fencing_version;
    assert!(second_fence > take.fencing_version, "再次执行使用新的 fence");
    fail_unknown(&store, claim).await;
    store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须收口");
    assert_eq!(reason(&store, case).await, Some(Reason::AwaitingObservation));
    let version = case_version(pool, case).await;
    assert_eq!(
        rejected(retry(&store, case, version, Uuid::new_v4()).await),
        SettlementReturnError::ObservationRequired,
        "旧观察不能授权新一轮"
    );
    assert!(
        !record(&store, &take, take.fencing_version, &session.binding(), Reason::AssetsClaimable)
            .await
            .expect("旧 fence 的观察不报错"),
        "旧 fence 的观察不能留证"
    );
    assert!(
        record(&store, &take, second_fence, &session.binding(), Reason::AssetsClaimable)
            .await
            .expect("新 fence 留证")
    );
    let version = case_version(pool, case).await;
    retry(&store, case, version, Uuid::new_v4()).await.expect("再次领回");

    // 第三次执行成功：按正常领回证明收口代管，领回事实与资金返还各一次，金额仍为 M。
    let claim = take_into_sending(&store, take.take_delivery, &take.expectation).await;
    store
        .succeed(
            claim,
            DeliverySendResult {
                external_reference: format!("mooglemail:main:take:{}", take.mid),
                result_metadata: json!({
                    "operation": "take",
                    "external_mail_id": take.mid,
                    "proof": "action_removed_or_success_page",
                    "expected_credits": PAYOUT,
                    "expected_cod": 0,
                    "expected_attachment_ids": [],
                }),
            },
            Timestamp::now(),
        )
        .await
        .expect("Take 成功收口");
    let settled =
        store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须收口");
    assert_eq!(settled.status, SettlementReturnCaseStatus::Held);
    let done = facts(pool, &take).await;
    assert_eq!((done.takes, done.funding_returns, done.return_entries), (1, 1, 1));
    assert_eq!((done.allocation_state.as_str(), done.allocation_amount), ("available", PAYOUT));

    pool.close().await;
}

/// 观察留证之后原凭据换了版本（停用再启用、改名或远端失效都会推进版本，且回不到原版本）：案件
/// 最近一次事件仍是当前 fence 上“完整仍在且可领取”的观察，再次领回照常受理——准入不比较凭据
/// 版本，资产是否仍在由执行前的预检在当时的会话里重新核对。新一轮执行冻结的是执行当时的凭据
/// 身份：仍以原版本准备的发送围栏被拒绝，以当前版本准备的才进入 sending。此后结果未知的复核只
/// 认新冻结的会话：原版本的观察不再留证，当前版本的观察才留证并授权下一次再领回。
#[tokio::test]
async fn postgres_return_take_retry_after_credential_change_freezes_the_current_credential() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let take = unknown_take(
        &store,
        &session,
        7_640_041,
        "996441",
        SettlementReturnAction::ReclaimAndHold,
        true,
    )
    .await;
    let case = take.returned.case;
    assert!(
        record(&store, &take, take.fencing_version, &session.binding(), Reason::AssetsClaimable)
            .await
            .expect("留证")
    );
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");
    sqlx::query("UPDATE public.credential_keys SET state_version = 2 WHERE id = $1")
        .bind(session.credential)
        .execute(maintenance.postgres_pool())
        .await
        .expect("原凭据换版本");
    maintenance.postgres_pool().close().await;

    // 最近一次事件仍是可领取的观察：原因不变，再次领回照常受理。
    let detail = store.settlement_return_detail(case).await.expect("案件详情");
    assert_eq!(detail.take_unknown_reason, Some(Reason::AssetsClaimable));
    assert_eq!(detail.allowed_actions, [SettlementReturnAction::RetryTake]);
    let result =
        retry(&store, case, detail.summary.case_version, Uuid::new_v4()).await.expect("再次领回");
    assert_eq!(result.status, SettlementReturnCaseStatus::TakePending);

    // 原版本的身份已不是同版本的 active 凭据：以它准备的执行过不了发送围栏，没有进入 sending。
    let claim = claim_take(&store, take.take_delivery).await;
    let second_fence = claim.fencing_version;
    assert!(second_fence > take.fencing_version, "再次执行使用新的 fence");
    let stale = DeliveryPreparedExpectation::Take(Arc::new(take.expectation.clone()));
    let outcome = store
        .start_sending_authorized(claim.clone(), Timestamp::now(), None, Some(&stale))
        .await
        .expect("发送围栏结论必须确定");
    assert_ne!(outcome, DeliveryFenceOutcome::Sending, "原版本身份不能进入 sending");
    assert_ne!(facts(pool, &take).await.delivery_status, "sending");

    // 以当前版本准备的才进入 sending；冻结的 Take 尝试记的是当前版本。
    let mut current = session.binding();
    current.credential_version = 2;
    let mut expectation = take.expectation.clone();
    expectation.credential = Some(current.clone());
    let prepared = DeliveryPreparedExpectation::Take(Arc::new(expectation.clone()));
    let outcome = store
        .start_sending_authorized(claim.clone(), Timestamp::now(), None, Some(&prepared))
        .await
        .expect("发送围栏结论必须确定");
    assert_eq!(outcome, DeliveryFenceOutcome::Sending, "当前版本身份进入 sending");
    let frozen_version: Option<String> = sqlx::query_scalar(
        "SELECT attempt.expectation -> 'credential' ->> 'credential_version'
         FROM public.delivery_current_take_attempt($1) AS attempt",
    )
    .bind(take.take_delivery)
    .fetch_one(pool)
    .await
    .expect("读取冻结的 Take 尝试");
    assert_eq!(frozen_version.as_deref(), Some("2"), "新一轮执行冻结当前凭据版本");

    // 新一轮结果未知后，复核只认新冻结的会话。
    fail_unknown(&store, claim).await;
    store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须收口");
    assert_eq!(reason(&store, case).await, Some(Reason::AwaitingObservation));
    let observe = |expectation, binding| {
        store.record_return_take_observation(
            take.take_delivery,
            second_fence,
            Reason::AssetsClaimable,
            2,
            expectation,
            binding,
        )
    };
    let original = session.binding();
    assert!(observe(&take.expectation, &original).await.is_err(), "原版本会话的观察不再留证");
    assert!(observe(&expectation, &original).await.is_err(), "观察凭据与新冻结的身份不符时不留证");
    let version = case_version(pool, case).await;
    assert_eq!(
        rejected(retry(&store, case, version, Uuid::new_v4()).await),
        SettlementReturnError::ObservationRequired,
        "被拒绝的观察不能授权再次领回"
    );
    assert!(observe(&expectation, &current).await.expect("当前会话留证"));
    let version = case_version(pool, case).await;
    retry(&store, case, version, Uuid::new_v4()).await.expect("新会话的观察授权再次领回");

    pool.close().await;
}

/// 观察证明成立时同邮件互斥仍生效：另一 owner 对同一邮件有处置意图时再次领回整体回滚。
#[tokio::test]
async fn postgres_return_take_retry_respects_mail_disposition_exclusion() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let take = unknown_take(
        &store,
        &session,
        7_640_031,
        "996431",
        SettlementReturnAction::ReclaimAndHold,
        true,
    )
    .await;
    let case = take.returned.case;
    assert!(
        record(&store, &take, take.fencing_version, &session.binding(), Reason::AssetsClaimable)
            .await
            .expect("留证")
    );
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count, completed_at, failure_code)
         VALUES ($1, $2, decode(repeat('5e', 32), 'hex'), 'mail_intake', $3, 'MoogleMail',
                 jsonb_build_object('schema_version', 1, 'operation', 'return', 'world', 'main',
                     'external_mail_id', $4::text),
                 'failed', 1, 1, now(), 'remote_rejected')",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().to_string())
    .bind(&take.mid)
    .execute(pool)
    .await
    .expect("写入同邮件的其他处置意图");
    let before = facts(pool, &take).await;
    assert_eq!(
        rejected(retry(&store, case, before.case_version, Uuid::new_v4()).await),
        SettlementReturnError::ActionConflict
    );
    assert_eq!(facts(pool, &take).await, before, "互斥成立时不改写任何事实");

    pool.close().await;
}
