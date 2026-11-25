//! 付款提醒（7A-2 remind）：每合同一次，重放返回原结果；派前复验，可能已派的零补发；
//! 从未进入 sending 的作废提醒在领取前扫描中以预检退役收口（7A-3）。
//!
//! 保护的不变量：1（命令幂等）、3（决定、投递与回执同事务）、4（外部写不自动重发）、
//! 5（投递终态持久化，不靠反复领取）。

use std::sync::Arc;

use auction_application::consignment::{
    CONSIGNMENT_PAYMENT_REMINDER_SUBJECT, PaymentReminderService, PaymentReminderStore,
    ReminderOutcome,
};
use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryFenceOutcome, DeliveryPreparedExpectation,
    DeliverySendExpectation, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_application::operations::{
    AuditFailureMode, DispositionReadMode, OperationsAlertCollectionOverdueRead,
    OperationsAlertSourceReader,
};
use auction_application::runtime_switches::{RuntimeSwitchRole, RuntimeSwitches};
use auction_domain::consignment::recovery::ReminderDecision;
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::CONTRACT;
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{original_dispatch_sql, seed_purchased};
use super::super::ledger136::funding::{pay, seed as seed_unpaid};
use crate::support::TestDatabase;

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取计数")
}

/// 到期且已证寄出、仍未付：每合同只排一封通用投递；重放返回原结果；派出前复验通过，
/// 投递一旦离开 pending（可能已派）就零补发。
#[tokio::test]
async fn postgres_consignment_recovery_reminder_is_queued_once_and_never_resent() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_purchased(pool).await;
    succeeds(pool, &original_dispatch_sql()).await;
    let port: Arc<dyn PaymentReminderStore> = store.clone();
    let service = PaymentReminderService::new(port);
    let contract = CONTRACT.parse().expect("合同 ID");

    assert_eq!(service.due(None, 10).await.expect("待办").items, vec![contract]);
    // 键集分页：读满一页给出游标，游标之后没有更多待办（正例）；读不满一页没有游标（反例）。
    let page = service.due(None, 1).await.expect("满页");
    assert_eq!(page.items, vec![contract]);
    let next = page.next.expect("读满一页应给出游标");
    let after = service.due(Some(next), 1).await.expect("游标之后");
    assert!((after.items.is_empty(), after.next.is_none()) == (true, true), "{after:?}");
    assert_eq!(service.due(None, 10).await.expect("未满页").next, None);
    let first = service.remind(contract).await.expect("排提醒");
    assert!(!first.replayed);
    let ReminderOutcome::Queued { delivery_id } = first.outcome else {
        panic!("到期且已证寄出、未付的合同应排出提醒: {:?}", first.outcome);
    };
    let deliveries =
        "SELECT count(*) FROM public.deliveries WHERE origin_kind = 'consignment_notice'";
    let reminders = "SELECT count(*) FROM public.consignment_payment_reminders";
    assert_eq!((count(pool, deliveries).await, count(pool, reminders).await), (1, 1));

    // 每合同一次：重放得到原投递，不新增行；已有决定后不再出现在待办里。
    let replay = service.remind(contract).await.expect("重放");
    assert!(replay.replayed);
    assert_eq!(replay.outcome, ReminderOutcome::Queued { delivery_id });
    assert_eq!((count(pool, deliveries).await, count(pool, reminders).await), (1, 1));
    assert!(service.due(None, 10).await.expect("待办").items.is_empty());

    // 派前复验：尚未派出（pending）与已领取但未进 sending（claimed）都可继续；
    // 一旦进入 sending（可能已派出）就零补发。
    assert_eq!(
        store.test_reminder_pre_dispatch(contract).await.expect("复验"),
        ReminderDecision::Queue
    );
    succeeds(
        pool,
        &format!(
            "UPDATE public.deliveries
             SET status = 'claimed', lease_token = gen_random_uuid(),
                 lease_expires_at = timestamptz '2099-01-01T00:00:00Z',
                 fencing_version = 1, attempt_count = 1
             WHERE id = '{delivery_id}';"
        ),
    )
    .await;
    assert_eq!(
        store.test_reminder_pre_dispatch(contract).await.expect("复验"),
        ReminderDecision::Queue
    );
    succeeds(
        pool,
        &format!(
            "UPDATE public.deliveries
             SET status = 'sending', sending_at = statement_timestamp()
             WHERE id = '{delivery_id}';"
        ),
    )
    .await;
    assert_eq!(
        store.test_reminder_pre_dispatch(contract).await.expect("复验"),
        ReminderDecision::NoResend
    );
    assert_eq!((count(pool, deliveries).await, count(pool, reminders).await), (1, 1), "零补发");
}

/// 领取已排队的提醒投递，并以它的真实行组装 claim（围栏的拒绝只能来自提醒复验，而不是 claim 不符）。
async fn claim_reminder(
    runtime: &DatabaseStore,
    pool: &PgPool,
    delivery_id: Uuid,
) -> DeliveryWorkClaim {
    let now = Timestamp::now();
    let grace = std::time::Duration::from_secs(60);
    runtime.begin_claim_cycle(now, grace).await.expect("领取前清理");
    let claims = runtime
        .claim_due(
            "reminder-fence".to_owned(),
            now,
            now + Duration::minutes(2),
            grace,
            DeliveryClaimExclusions::default(),
            10,
        )
        .await
        .expect("领取投递");
    let claim = claims
        .into_iter()
        .find(|claim| claim.delivery_id == delivery_id)
        .expect("提醒投递必须能被通用领取");
    assert_eq!(claim.payload["template"], "consignment_payment_reminder");
    let status: String = sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(delivery_id)
        .fetch_one(pool)
        .await
        .expect("投递状态");
    assert_eq!(status, "claimed");
    claim
}

fn reminder_expectation(claim: &DeliveryWorkClaim) -> DeliveryPreparedExpectation {
    DeliveryPreparedExpectation::Send(Arc::new(DeliverySendExpectation {
        world_id: "main".to_owned(),
        recipient: claim.recipient.clone(),
        subject: CONSIGNMENT_PAYMENT_REMINDER_SUBJECT.to_owned(),
        body_digest: [0xd1; 32],
        credits: 0,
        cod: 0,
        equipment_ids: Vec::new(),
        credential: None,
    }))
}

/// 排好提醒并领取；`paid` 为真时在排队之后、围栏之前买家完成付款。
async fn queued_reminder_fence(paid: bool) -> (DeliveryFenceOutcome, String, i64) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let runtime = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_unpaid(pool).await;
    let port: Arc<dyn PaymentReminderStore> = runtime.clone();
    let service = PaymentReminderService::new(port);
    let receipt = service.remind(CONTRACT.parse().expect("合同 ID")).await.expect("排提醒");
    let ReminderOutcome::Queued { delivery_id } = receipt.outcome else {
        panic!("应排出提醒: {:?}", receipt.outcome);
    };
    let claim = claim_reminder(&runtime, pool, delivery_id).await;
    let expectation = reminder_expectation(&claim);
    if paid {
        pay(pool).await;
    }
    let outcome = runtime
        .start_sending_authorized(claim, Timestamp::now(), None, Some(&expectation))
        .await
        .expect("围栏必须有确定结论");
    let status: String = sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(delivery_id)
        .fetch_one(pool)
        .await
        .expect("投递状态");
    let attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("尝试数");
    (outcome, status, attempts)
}

/// 派前复验的执行点在通用投递的 sending 围栏：未付款的提醒可以进入 sending（正例）；
/// 排队之后买家已付款则围栏拒绝，投递仍是 claimed、没有任何发送尝试（零发送，反例）。
#[tokio::test]
async fn postgres_consignment_recovery_reminder_fence_voids_a_paid_reminder_before_sending() {
    let (outcome, status, attempts) = queued_reminder_fence(false).await;
    assert_eq!(outcome, DeliveryFenceOutcome::Sending);
    assert_eq!((status.as_str(), attempts), ("sending", 1));

    let (outcome, status, attempts) = queued_reminder_fence(true).await;
    assert_eq!(outcome, DeliveryFenceOutcome::Deferred, "已付款的提醒不得进入 sending");
    assert_eq!((status.as_str(), attempts), ("claimed", 0), "零发送：没有尝试、没有 sending");
}

/// 排好一封未付合同的提醒并由通用链领取（领取前扫描已运行一次：此时未付，提醒不得被退役）。
async fn claimed_reminder() -> (TestDatabase, DatabaseStore, Arc<DatabaseStore>, DeliveryWorkClaim)
{
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let runtime = Arc::new(database.store().await.expect("运行账号"));
    seed_unpaid(owner.postgres_pool()).await;
    let port: Arc<dyn PaymentReminderStore> = runtime.clone();
    let service = PaymentReminderService::new(port);
    let receipt = service.remind(CONTRACT.parse().expect("合同 ID")).await.expect("排提醒");
    let ReminderOutcome::Queued { delivery_id } = receipt.outcome else {
        panic!("应排出提醒: {:?}", receipt.outcome);
    };
    let claim = claim_reminder(&runtime, owner.postgres_pool(), delivery_id).await;
    (database, owner, runtime, claim)
}

/// 分类口径的告警快照：(pending, claimed, 投递未处置失败, 通知未处置失败)。
async fn classified_counts(store: &DatabaseStore) -> (i64, i64, i64, i64) {
    let classified = store.clone().with_runtime_switches(
        RuntimeSwitches::new(RuntimeSwitchRole::Worker, AuditFailureMode::Shadow, true)
            .with_disposition_read_mode_baseline(DispositionReadMode::Classified),
    );
    let snapshot = classified
        .snapshot(Timestamp::now(), OperationsAlertCollectionOverdueRead::Skip)
        .await
        .expect("读取告警快照");
    let delivery = snapshot.delivery.expect("投递水位");
    let notification = snapshot.notification.expect("通知水位");
    (delivery.pending, delivery.claimed, delivery.failed, notification.failed_count)
}

/// 投递的 (状态, 失败码, 是否进过 sending, 发送尝试数)。
async fn delivery_row(pool: &PgPool, delivery_id: Uuid) -> (String, Option<String>, bool, i64) {
    sqlx::query_as(
        "SELECT delivery.status, delivery.failure_code, delivery.sending_at IS NOT NULL,
                (SELECT count(*) FROM public.delivery_send_attempts AS attempt
                 WHERE attempt.delivery_id = delivery.id)
         FROM public.deliveries AS delivery WHERE delivery.id = $1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("投递行")
}

/// 投递当前 fence 上的处置事实 (种类, 原因码, 是否解除计数)。
async fn current_dispositions(
    pool: &PgPool,
    delivery_id: Uuid,
) -> Vec<(String, Option<String>, bool)> {
    sqlx::query_as(
        "SELECT disposition.kind, disposition.reason_code, disposition.resolves_alert
         FROM public.delivery_dispositions AS disposition
         JOIN public.deliveries AS delivery ON delivery.id = disposition.delivery_id
         WHERE disposition.delivery_id = $1
           AND disposition.fencing_version = delivery.fencing_version",
    )
    .bind(delivery_id)
    .fetch_all(pool)
    .await
    .expect("处置事实")
}

/// 租约过期后再领取一轮；返回是否又领到了这封提醒。
async fn reclaimed(runtime: &DatabaseStore, delivery_id: Uuid) -> bool {
    let later = Timestamp::now() + Duration::hours(1);
    let grace = std::time::Duration::from_secs(60);
    runtime.begin_claim_cycle(later, grace).await.expect("领取前清理");
    runtime
        .claim_due(
            "reminder-retired".to_owned(),
            later,
            later + Duration::minutes(2),
            grace,
            DeliveryClaimExclusions::default(),
            10,
        )
        .await
        .expect("领取投递")
        .iter()
        .any(|claim| claim.delivery_id == delivery_id)
}

/// 正例：排队并领取后买家付款，围栏拒绝（零发送）；下一轮领取前扫描在同一事务内把提醒作废并退役投递：
/// failed、预检退役处置解除计数，不再被领取，积压不含它，分类口径不计失败，没有新增发送尝试或命令。
#[tokio::test]
async fn postgres_consignment_recovery_reminder_voided_before_sending_is_retired() {
    let (_database, owner, runtime, claim) = claimed_reminder().await;
    let pool = owner.postgres_pool();
    let delivery_id = claim.delivery_id;
    let expectation = reminder_expectation(&claim);
    let before = classified_counts(&owner).await;
    let commands = "SELECT count(*) FROM public.consignment_commands";
    let commands_before = count(pool, commands).await;

    pay(pool).await;
    let outcome = runtime
        .start_sending_authorized(claim, Timestamp::now(), None, Some(&expectation))
        .await
        .expect("围栏必须有确定结论");
    assert_eq!(outcome, DeliveryFenceOutcome::Deferred, "已付款的提醒不得进入 sending");
    // 存量形态：围栏拒绝后投递仍停在 claimed；下一轮领取前扫描收口。
    assert_eq!(delivery_row(pool, delivery_id).await.0, "claimed");
    runtime
        .begin_claim_cycle(Timestamp::now(), std::time::Duration::from_secs(60))
        .await
        .expect("扫描");

    assert_eq!(
        delivery_row(pool, delivery_id).await,
        ("failed".to_owned(), Some("consignment_notice_invalidated".to_owned()), false, 0),
        "作废提醒退役为 failed，从未进入 sending，零发送尝试"
    );
    assert_eq!(
        current_dispositions(pool, delivery_id).await,
        vec![(
            "preflight_retired".to_owned(),
            Some("consignment_notice_invalidated".to_owned()),
            true
        )]
    );
    assert!(!reclaimed(&runtime, delivery_id).await, "退役后不得再被领取");
    let after = classified_counts(&owner).await;
    assert_eq!(
        after,
        (before.0 - 1, before.1 - 1, before.2, before.3),
        "积压（快照的 pending 含已领取未完成的行）去掉这封提醒，分类口径的投递与通知失败数不变"
    );
    assert_eq!(count(pool, commands).await, commands_before, "退役不写任何命令");
}

/// 反例：提醒已进入 sending（可能已派出）后买家才付款，领取前扫描不得终结它，也不补发；
/// 同一流程里扫描先于领取运行时提醒未付，照常进入 sending（未作废的提醒照常发送）。
#[tokio::test]
async fn postgres_consignment_recovery_reminder_already_sending_is_not_retired_or_resent() {
    let (_database, owner, runtime, claim) = claimed_reminder().await;
    let pool = owner.postgres_pool();
    let delivery_id = claim.delivery_id;
    let expectation = reminder_expectation(&claim);
    let outcome = runtime
        .start_sending_authorized(claim, Timestamp::now(), None, Some(&expectation))
        .await
        .expect("围栏必须有确定结论");
    assert_eq!(outcome, DeliveryFenceOutcome::Sending, "未作废的提醒照常进入 sending");

    pay(pool).await;
    runtime
        .begin_claim_cycle(Timestamp::now(), std::time::Duration::from_secs(60))
        .await
        .expect("扫描");

    assert_eq!(
        delivery_row(pool, delivery_id).await,
        ("sending".to_owned(), None, true, 1),
        "已进入 sending 的提醒不被终结，只有原来那一次发送尝试"
    );
    assert!(current_dispositions(pool, delivery_id).await.is_empty());
}
