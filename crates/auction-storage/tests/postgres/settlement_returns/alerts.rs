//! 结算退件案件的导航待办计数与运维告警（迁移 000114、000116）在 PostgreSQL 18 上的用例，以运行
//! 账号执行：
//! - 导航待办 `settlement_returns` 按未结案案件计数（含隔离、待核验与 Take 结果未知），不要求存在
//!   义务，结案后回落；
//! - `settlement_return_pending`（已绑定待处理）、`settlement_return_quarantined`（隔离或待核验）与
//!   `settlement_return_take_unknown`（Take 结果未知，作用域为具体原因）由扫描按未结案案件写入；
//!   告警确认不改变案件，案件全部结案后由下一轮扫描关闭；
//! - 案件读取失败时履约来源不可用：已有退件告警保持活动，不当作零件关闭，待办计数读取直接失败。
//!
//! Worker 一轮扫描以 `snapshot` → `evaluate_snapshot` → `observe` 模拟；Take 的外部执行以直接 SQL
//! 模拟终态。不发外部请求。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration as StdDuration;

use auction_application::operations::{
    OperationsAlertAcknowledgeCommand, OperationsAlertCollectionOverdueRead,
    OperationsAlertConsole, OperationsAlertObserveCommand, OperationsAlertPolicy,
    OperationsAlertRuntimeStatus, OperationsAlertSourceReader, OperationsAlertStore,
    evaluate_snapshot,
};
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{
    SettlementReturnAdminPort, SettlementReturnCommand, SettlementReturnWorkStore,
};
use auction_domain::{CommandId, SettlementReturnAction, SettlementReturnCaseStatus};
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::{evidence, insert_inbound, insert_sent_fulfillment};
use crate::recovery_returns::{insert_sale, settlement_minute};
use crate::support;

const PAYOUT: i64 = 90;

fn policy() -> OperationsAlertPolicy {
    OperationsAlertPolicy {
        source_timeout: StdDuration::from_secs(5),
        batch_window: StdDuration::from_secs(120),
        cooldown: StdDuration::from_secs(1_800),
        claim_lease: StdDuration::from_secs(120),
        retry_delay: StdDuration::from_secs(300),
        max_signals_per_scan: 64,
        max_batch_items: 20,
        max_body_bytes: 4_096,
        delivery_pending_count_threshold: 1,
        delivery_pending_age_threshold: StdDuration::from_secs(900),
        delivery_manual_reconcile_count_threshold: 1,
        fulfillment_backlog_count_threshold: 1,
        fulfillment_backlog_age_threshold: StdDuration::from_secs(900),
        fulfillment_collection_overdue_age_threshold: StdDuration::from_secs(259_200),
        recovery_overdue_count_threshold: 1,
        recovery_overdue_age_threshold: StdDuration::from_secs(900),
    }
}

/// 已寄出货款被系统退回、已绑定原派发的案件（待决定）；返回案件 ID。
async fn bound_return(store: &auction_storage::DatabaseStore, sequence: i64, mid: &str) -> Uuid {
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, sequence, settled_at, 1).await;
    insert_sent_fulfillment(
        pool,
        sale.seller_obligation,
        "settlement_seller_payout",
        PAYOUT,
        minute + Duration::minutes(10),
        "enqueued",
    )
    .await;
    let case = insert_inbound(
        pool,
        mid,
        minute + Duration::minutes(40),
        Some(("MoogleMail", format!("[RTS] Auction seller payout [{}]", sale.seller_obligation))),
        evidence(sale.seller_obligation, PAYOUT, &[]),
    )
    .await;
    store.consume_due_settlement_return(Timestamp::now()).await.expect("消费").expect("必须消费");
    case
}

/// 两件隔离退件：非系统发件人（untrusted）与未知义务（unbound）；返回两个案件 ID。
async fn quarantined_returns(store: &auction_storage::DatabaseStore) -> (Uuid, Uuid) {
    let pool = store.postgres_pool();
    let (minute, _) = settlement_minute();
    let unknown = Uuid::new_v4();
    let untrusted = insert_inbound(
        pool,
        "994101",
        minute + Duration::minutes(41),
        Some(("Player", format!("[RTS] Auction buyer refund [{unknown}]"))),
        evidence(unknown, 5, &[]),
    )
    .await;
    let unbound = insert_inbound(
        pool,
        "994102",
        minute + Duration::minutes(42),
        Some(("MoogleMail", format!("[RTS] Auction buyer refund [{unknown}]"))),
        evidence(unknown, 5, &[]),
    )
    .await;
    while store.consume_due_settlement_return(Timestamp::now()).await.expect("消费").is_some() {}
    (untrusted, unbound)
}

async fn handle(
    store: &auction_storage::DatabaseStore,
    case: Uuid,
    action: SettlementReturnAction,
) -> SettlementReturnCaseStatus {
    let version =
        store.settlement_return_detail(case).await.expect("读取案件").summary.case_version;
    let command = SettlementReturnCommand::new(
        CommandId::from_uuid(Uuid::new_v4()),
        case,
        version,
        action,
        "admin-returns",
        Timestamp::now(),
    )
    .expect("构造命令");
    store.handle_settlement_return(command).await.expect("执行退件命令").status
}

/// 模拟 Worker 一轮告警扫描：读取快照、评估并写入观察。
pub(super) async fn scan(store: &auction_storage::DatabaseStore) {
    let now = Timestamp::now();
    let snapshot = store
        .snapshot(now, OperationsAlertCollectionOverdueRead::Skip)
        .await
        .expect("读取告警快照");
    let observations = evaluate_snapshot(&snapshot, &policy()).expect("扫描评估");
    store
        .observe(OperationsAlertObserveCommand {
            observations,
            now,
            batch_window: StdDuration::from_secs(120),
        })
        .await
        .expect("写入扫描观察");
}

/// 履约来源下某种 signal 的（是否活动，计数）；没有行时为 `None`。
pub(super) async fn signal(pool: &PgPool, kind: &str, scope: &str) -> Option<(bool, i64)> {
    sqlx::query_as(
        "SELECT active, count_value FROM public.operations_alert_signals
         WHERE kind = $1 AND source = 'fulfillment' AND scope = $2",
    )
    .bind(kind)
    .bind(scope)
    .fetch_optional(pool)
    .await
    .expect("读取告警信号")
}

pub(super) async fn enable_alerts(store: &auction_storage::DatabaseStore) {
    store
        .publish_runtime_status(OperationsAlertRuntimeStatus {
            enabled: true,
            updated_at: Timestamp::now(),
        })
        .await
        .expect("启用告警扫描");
}

/// 已绑定待决定与两件隔离案件分别进入两类告警，导航待办按未结案案件计 3；确认告警不改变案件；
/// 案件全部结案后待办归零，下一轮扫描关闭两类告警。
#[tokio::test]
async fn postgres_settlement_return_alerts_follow_open_cases_and_close_on_closure() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let bound = bound_return(&store, 7_700_001, "994001").await;
    let (untrusted, unbound) = quarantined_returns(&store).await;

    let counts = store.pending_counts().await.expect("导航待办");
    assert_eq!(counts.settlement_returns, 3, "隔离记录与未知义务也计入待办");
    assert_eq!(counts.delivery_manual_reconcile, 0, "退件不塞进投递人工对账计数");

    enable_alerts(&store).await;
    scan(&store).await;
    assert_eq!(signal(pool, "settlement_return_pending", "auction").await, Some((true, 1)));
    assert_eq!(signal(pool, "settlement_return_quarantined", "auction").await, Some((true, 2)));
    let snapshot = store
        .snapshot(Timestamp::now(), OperationsAlertCollectionOverdueRead::Skip)
        .await
        .expect("读取告警快照");
    let fulfillment = snapshot.fulfillment.expect("履约水位");
    assert_eq!(fulfillment.manual_reconcile, 0, "由案件接管的人工对账不重复计数");

    // 告警确认只追加操作者事实，不改变案件；下一轮扫描仍按未结案案件保持活动。
    let (fingerprint, incident_generation): (Vec<u8>, i64) = sqlx::query_as(
        "SELECT fingerprint, webhook_generation FROM public.operations_alert_signals
         WHERE kind = 'settlement_return_pending' AND source = 'fulfillment'",
    )
    .fetch_one(pool)
    .await
    .expect("读取告警指纹");
    let fingerprint: [u8; 32] = fingerprint.try_into().expect("指纹 32 字节");
    let acknowledge = OperationsAlertAcknowledgeCommand::new(
        Uuid::new_v4(),
        fingerprint,
        incident_generation,
        "operator",
        "核对中",
    )
    .expect("构造确认命令");
    store.acknowledge_alert(acknowledge).await.expect("确认告警");
    let detail = store.settlement_return_detail(bound).await.expect("案件详情");
    assert_eq!(detail.summary.status, SettlementReturnCaseStatus::AwaitingDecision);
    scan(&store).await;
    assert_eq!(signal(pool, "settlement_return_pending", "auction").await, Some((true, 1)));

    // 隔离记录核对后结案：隔离告警关闭，已绑定案件仍待处理。
    assert_eq!(
        handle(&store, untrusted, SettlementReturnAction::Acknowledge).await,
        SettlementReturnCaseStatus::Reviewed
    );
    assert_eq!(
        handle(&store, unbound, SettlementReturnAction::Acknowledge).await,
        SettlementReturnCaseStatus::Reviewed
    );
    scan(&store).await;
    assert_eq!(
        signal(pool, "settlement_return_quarantined", "auction").await.map(|row| row.0),
        Some(false)
    );
    assert_eq!(signal(pool, "settlement_return_pending", "auction").await, Some((true, 1)));
    assert_eq!(store.pending_counts().await.expect("导航待办").settlement_returns, 1);

    // 留箱结案后待办归零、待处理告警关闭；权益保留，履约以明确原因终结不再计失败。
    assert_eq!(
        handle(&store, bound, SettlementReturnAction::CloseInbox).await,
        SettlementReturnCaseStatus::InboxClosed
    );
    scan(&store).await;
    assert_eq!(
        signal(pool, "settlement_return_pending", "auction").await.map(|row| row.0),
        Some(false)
    );
    assert_eq!(store.pending_counts().await.expect("导航待办").settlement_returns, 0);
    let fulfillment = store
        .snapshot(Timestamp::now(), OperationsAlertCollectionOverdueRead::Skip)
        .await
        .expect("读取告警快照")
        .fulfillment
        .expect("履约水位");
    assert_eq!(
        (
            fulfillment.settlement_return_pending,
            fulfillment.settlement_return_quarantined,
            fulfillment.failed
        ),
        (0, 0, 0)
    );

    pool.close().await;
}

/// 领回中的案件仍是待处理；Take 结果未知后转入按原因分作用域的结果未知告警（这件 Take 没有冻结
/// 凭据身份的尝试记录，原因是 `session_changed`）并继续计入待办，不计入隔离告警；只认系统可验证
/// 证明，不会被扫描当作已结案关闭。
#[tokio::test]
async fn postgres_take_unknown_moves_case_to_take_unknown_alert() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let case = bound_return(&store, 7_700_011, "994011").await;
    enable_alerts(&store).await;

    assert_eq!(
        handle(&store, case, SettlementReturnAction::ReclaimAndHold).await,
        SettlementReturnCaseStatus::TakePending
    );
    scan(&store).await;
    assert_eq!(signal(pool, "settlement_return_pending", "auction").await, Some((true, 1)));
    assert_eq!(signal(pool, "settlement_return_quarantined", "auction").await, None);

    store
        .activate_due_return_take(Timestamp::now(), super::take_worlds())
        .await
        .expect("激活")
        .expect("必须激活");
    let take: Uuid = sqlx::query_scalar(
        "SELECT delivery_id FROM public.settlement_return_actions WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取 Take 投递");
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'manual_reconcile', fencing_version = 1, attempt_count = 1,
             claimed_at = now(), sending_at = now(), completed_at = now(), updated_at = now(),
             failure_code = 'remote_unknown', result_metadata = '{}'::jsonb
         WHERE id = $1",
    )
    .bind(take)
    .execute(pool)
    .await
    .expect("模拟 Take 结果未知");
    store.settle_due_return_take(Timestamp::now()).await.expect("收口 Take").expect("必须收口");
    let detail = store.settlement_return_detail(case).await.expect("案件详情");
    assert_eq!(detail.summary.status, SettlementReturnCaseStatus::TakeUnknown);
    assert!(detail.allowed_actions.is_empty(), "结果未知没有人工声明或重试入口");

    assert_eq!(
        detail.take_unknown_reason,
        Some(auction_domain::SettlementReturnTakeUnknownReason::SessionChanged),
        "没有冻结凭据的 Take 无法在原会话复核"
    );

    scan(&store).await;
    assert_eq!(
        signal(pool, "settlement_return_take_unknown", "session_changed").await,
        Some((true, 1))
    );
    assert_eq!(signal(pool, "settlement_return_quarantined", "auction").await, None);
    assert_eq!(
        signal(pool, "settlement_return_pending", "auction").await.map(|row| row.0),
        Some(false)
    );
    assert_eq!(store.pending_counts().await.expect("导航待办").settlement_returns, 1);

    pool.close().await;
}

/// 案件表读取失败（运行账号失去 SELECT）时：履约来源不可用，已有退件告警保持活动而不是按零件
/// 关闭；导航待办计数读取直接失败，不回落为零。
#[tokio::test]
async fn postgres_settlement_return_read_failure_keeps_alert_open() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    bound_return(&store, 7_700_021, "994021").await;
    enable_alerts(&store).await;
    scan(&store).await;
    assert_eq!(signal(pool, "settlement_return_pending", "auction").await, Some((true, 1)));

    // 故障注入（维护账号，仅本测试库）：收回运行账号对案件表的读取。
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");
    sqlx::query("REVOKE SELECT ON public.settlement_return_cases FROM auction_runtime")
        .execute(maintenance.postgres_pool())
        .await
        .expect("收回案件表读取");
    maintenance.postgres_pool().close().await;

    let snapshot = store
        .snapshot(Timestamp::now(), OperationsAlertCollectionOverdueRead::Skip)
        .await
        .expect("其余来源照常读取");
    assert!(snapshot.fulfillment.is_none(), "履约水位读取失败时缺席，不给零值");
    assert!(store.pending_counts().await.is_err(), "待办计数读取失败不回落为零");

    scan(&store).await;
    assert_eq!(
        signal(pool, "settlement_return_pending", "auction").await,
        Some((true, 1)),
        "来源不可用不关闭已有退件告警"
    );
    assert_eq!(
        signal(pool, "source_unavailable", "fulfillment").await.map(|row| row.0),
        Some(true),
        "读取失败生成来源不可用"
    );

    pool.close().await;
}
