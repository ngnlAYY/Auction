//! 发送归属阻断的机器解除与被接管原投递的收口（PostgreSQL 18，运行账号）。
//!
//! - Sent MID 补充证明：计数规则因“发送不早于当前退件截止”失败（这里是退件之后确定未外写的
//!   一次尝试），Worker 自动推进以 `send_unattributed` 阻断；之后 Sent 恢复为被退回的那次发送补上
//!   远端 MID，到复核时间时 Worker 在锁内复核成立、解除阻断并在同一事务退回卖家。
//! - 退件到达前被重新排队、因恢复接管而停在待发的原付款投递：Admin 确认收回或 Worker 自动退回
//!   卖家时，恢复 owner 同事务把它终结为 `failed/recovery_superseded`，`cod_returned` 处置落在
//!   终结后的 fence 上；已进入发送围栏的投递仍按未解决外部写入拒绝。

use auction_application::core::ApplicationError;
use auction_application::delivery::{DeliveryAdminListRequest, DeliveryAdminPort};
use auction_application::operations::{
    AuditFailureMode, DispositionReadMode, OperationsAlertCollectionOverdueRead,
    OperationsAlertSourceReader,
};
use auction_application::recovery::{
    RecoveryCommandError, RecoveryCommandRepository, RecoveryConfirmationAction,
    RecoveryResolutionStore,
};
use auction_application::runtime_switches::{RuntimeSwitchRole, RuntimeSwitches};
use auction_application::settlement::FundingReconciliationReader;
use auction_storage::DatabaseStore;
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::binding::{created_at, freeze_current_attempt, insert_attempt};
use super::fixture::{Sale, SaleSpec, insert_reclaiming_sale};
use super::{confirmation, dispositions, fulfillment};
use crate::support;

/// 达到冻结上限、拉黑已完成的 main 回收中成交：Worker 自动退回卖家，Admin 也可确认收回。
async fn sale(store: &DatabaseStore, sequence: i64) -> Sale {
    insert_reclaiming_sale(
        store.postgres_pool(),
        SaleSpec {
            world: "main",
            sequence,
            cod_return_count: 3,
            cod_return_limit: 3,
            blacklist_satisfied: true,
        },
    )
    .await
}

async fn recovery_state(pool: &PgPool, recovery_id: Uuid) -> (String, Option<String>, i64) {
    sqlx::query_as(
        "SELECT stage, blocking_code, state_version FROM public.settlement_recoveries
         WHERE id = $1",
    )
    .bind(recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取恢复记录")
}

async fn recovery_audits(pool: &PgPool, recovery_id: Uuid, action: &str) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT metadata FROM public.audit_records
         WHERE entity_type = 'settlement_recovery' AND entity_id = $1 AND action = $2
         ORDER BY occurred_at",
    )
    .bind(recovery_id.to_string())
    .bind(action)
    .fetch_all(pool)
    .await
    .expect("读取恢复审计")
}

/// 被退回的那次发送（fence 1，+5 分钟）留作冻结尝试；退件之后（+40 分钟）投递又以 fence 2 准备
/// 发送，但在写出前确定失败（明确记录 `write_started=false`、`outcome_unknown=false`）。
async fn retried_after_return(pool: &PgPool, sale: &Sale, at: DateTime<Utc>) {
    insert_attempt(pool, sale.payment_delivery, 1, at + Duration::minutes(5)).await;
    freeze_current_attempt(pool, sale.payment_delivery, 2, at + Duration::minutes(40)).await;
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'failed', failure_code = 'send_rejected', external_reference = NULL,
             completed_at = sending_at,
             result_metadata = '{\"operation\":\"send\",\"write_started\":false,\
                                 \"outcome_unknown\":false}'::jsonb
         WHERE id = $1",
    )
    .bind(sale.payment_delivery)
    .execute(pool)
    .await
    .expect("退件之后确定未外写的一次尝试");
}

/// 退件到达之前原付款投递被重新排队：被退回的那次发送留作冻结尝试（fence 2，+5 分钟），投递
/// 回到 pending（fence 3、从未进入发送围栏）。
pub(super) async fn requeued_before_return(pool: &PgPool, sale: &Sale, at: DateTime<Utc>) {
    freeze_current_attempt(pool, sale.payment_delivery, 2, at + Duration::minutes(5)).await;
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'pending', sending_at = NULL, claimed_at = NULL, completed_at = NULL,
             external_reference = NULL, result_metadata = '{}'::jsonb, fencing_version = 3
         WHERE id = $1",
    )
    .bind(sale.payment_delivery)
    .execute(pool)
    .await
    .expect("原付款投递改回待发");
}

async fn delivery_state(pool: &PgPool, delivery_id: Uuid) -> (String, Option<String>, i64, bool) {
    sqlx::query_as(
        "SELECT status, failure_code, fencing_version,
                COALESCE(result_metadata @> '{\"write_started\": false,
                                              \"outcome_unknown\": false}'::jsonb, false)
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("读取原付款投递")
}

async fn disposition_fences(pool: &PgPool, delivery_id: Uuid) -> Vec<(String, i64)> {
    sqlx::query_as(
        "SELECT kind, fencing_version FROM public.delivery_dispositions
         WHERE delivery_id = $1 ORDER BY occurred_at",
    )
    .bind(delivery_id)
    .fetch_all(pool)
    .await
    .expect("读取处置事实")
}

/// 计数规则不成立而阻断；Sent 恢复补上被退回那次发送的远端 MID 之后，到复核时间由 Worker 机器
/// 解除并在同一事务退回卖家。复核时间未到之前不重复复核。
#[tokio::test]
async fn postgres_sent_mid_attribution_releases_a_blocked_auto_return() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = sale(&store, 7_910_001).await;
    let at = created_at(pool, sale.recovery_id).await;
    retried_after_return(pool, &sale, at).await;

    let blocked = store
        .advance_due_cod_return(Utc::now())
        .await
        .expect("推进 COD 退回")
        .expect("到期记录必须被处理");
    assert_eq!((blocked.stage.as_str(), blocked.completed), ("reclaiming", false));
    let (_, code, blocked_version) = recovery_state(pool, sale.recovery_id).await;
    assert_eq!(code.as_deref(), Some("send_unattributed"));
    let reasons =
        recovery_audits(pool, sale.recovery_id, "recovery.send_attribution_blocked").await;
    assert_eq!(reasons.len(), 1);
    assert_eq!(reasons[0]["reason"], "send_after_return");

    // Sent 恢复为被退回的那次发送补上远端 MID（全局唯一）。
    sqlx::query(
        "INSERT INTO public.delivery_sent_bindings
            (world_id, external_mail_id, delivery_id, fencing_version, expectation_fencing_version)
         VALUES ('main', '881001', $1, 1, 1)",
    )
    .bind(sale.payment_delivery)
    .execute(pool)
    .await
    .expect("补上 Sent 绑定");
    assert!(
        store.advance_due_cod_return(Utc::now()).await.expect("推进 COD 退回").is_none(),
        "复核时间未到，不重复复核"
    );
    assert_eq!(recovery_state(pool, sale.recovery_id).await.2, blocked_version);

    sqlx::query(
        "UPDATE public.settlement_recoveries SET next_attempt_at = now() - interval '1 second'
         WHERE id = $1",
    )
    .bind(sale.recovery_id)
    .execute(pool)
    .await
    .expect("复核时间到达");
    let released = store
        .advance_due_cod_return(Utc::now())
        .await
        .expect("推进 COD 退回")
        .expect("复核成立后继续推进");
    assert_eq!(released.resolution.as_deref(), Some("returned_after_three_cod_returns"));
    assert!(released.completed);
    let (stage, code, _) = recovery_state(pool, sale.recovery_id).await;
    assert_eq!((stage.as_str(), code), ("completed", None));
    let releases =
        recovery_audits(pool, sale.recovery_id, "recovery.send_attribution_released").await;
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0]["rule"], "sent_mid");
    assert_eq!(
        fulfillment(pool, sale.payment).await,
        ("failed".to_owned(), Some("cod_returned_three_times".to_owned()))
    );

    pool.close().await;
}

/// 反例：被退回那次发送之外还有一次可能外达、没有 Sent MID 的发送时，复核仍不成立，只推迟下一次
/// 复核，不改状态版本、不解除阻断。
#[tokio::test]
async fn postgres_unprovable_send_keeps_the_attribution_block() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = sale(&store, 7_910_002).await;
    let at = created_at(pool, sale.recovery_id).await;
    retried_after_return(pool, &sale, at).await;
    // 同一轮另一次可能外达的发送（+6 分钟），结果已被覆盖、没有 Sent MID。
    insert_attempt(pool, sale.payment_delivery, 5, at + Duration::minutes(6)).await;
    sqlx::query(
        "INSERT INTO public.delivery_sent_bindings
            (world_id, external_mail_id, delivery_id, fencing_version, expectation_fencing_version)
         VALUES ('main', '881002', $1, 1, 1)",
    )
    .bind(sale.payment_delivery)
    .execute(pool)
    .await
    .expect("被退回那次发送的 Sent 绑定");

    store.advance_due_cod_return(Utc::now()).await.expect("推进").expect("阻断");
    let (_, code, version) = recovery_state(pool, sale.recovery_id).await;
    assert_eq!(code.as_deref(), Some("send_unattributed"));
    let reasons =
        recovery_audits(pool, sale.recovery_id, "recovery.send_attribution_blocked").await;
    assert_eq!(reasons[0]["reason"], "send_without_sent_mid");
    sqlx::query(
        "UPDATE public.settlement_recoveries SET next_attempt_at = now() - interval '1 second'
         WHERE id = $1",
    )
    .bind(sale.recovery_id)
    .execute(pool)
    .await
    .expect("复核时间到达");
    let rechecked =
        store.advance_due_cod_return(Utc::now()).await.expect("推进").expect("复核仍阻断");
    assert_eq!((rechecked.stage.as_str(), rechecked.state_version), ("reclaiming", version));
    let (_, code, after) = recovery_state(pool, sale.recovery_id).await;
    assert_eq!((code.as_deref(), after), (Some("send_unattributed"), version));
    let next_attempt: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT next_attempt_at FROM public.settlement_recoveries WHERE id = $1",
    )
    .bind(sale.recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取复核时间");
    assert!(next_attempt.is_some_and(|next| next > Utc::now()), "下一次复核推迟");
    assert!(
        recovery_audits(pool, sale.recovery_id, "recovery.send_attribution_released")
            .await
            .is_empty()
    );

    pool.close().await;
}

/// Admin 确认收回时，退件之前被重新排队、停在待发的原付款投递同事务终结；处置落在终结后的
/// fence 上，之后投递不再被领取。
#[tokio::test]
async fn postgres_reclaim_confirmation_retires_the_requeued_original() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = sale(&store, 7_910_003).await;
    let at = created_at(pool, sale.recovery_id).await;
    requeued_before_return(pool, &sale, at).await;

    let result = store
        .confirm_recovery(confirmation(&sale, RecoveryConfirmationAction::Reclaimed))
        .await
        .expect("被接管的待发投递不再阻挡确认");
    assert_eq!(result.resolution, "reclaimed");
    assert_superseded(pool, &sale, "admin").await;
    // 投递台：缺省投影不列出这一终态（已发布页面不认没有发送时间的失败），扩展投影列出且通过
    // 快照校验；详情读取同样通过校验。
    for (include, expected) in [(false, 0), (true, 1)] {
        let page = DeliveryAdminPort::list(
            &store,
            DeliveryAdminListRequest {
                reference: Some(sale.payment_delivery),
                include_settlement_return_actions: include,
                limit: 20,
                ..DeliveryAdminListRequest::default()
            },
        )
        .await
        .expect("读取投递台");
        assert_eq!((page.entries.len(), page.total), (expected, expected as i64), "{include}");
    }
    let detail =
        DeliveryAdminPort::detail(&store, sale.payment_delivery).await.expect("读取投递详情");
    assert_eq!(detail.delivery.failure_code.as_deref(), Some("recovery_superseded"));
    // 告警分类口径：终结后的投递由同 fence 的 `cod_returned` 处置解除，不计入投递失败。
    let snapshot = store
        .clone()
        .with_runtime_switches(
            RuntimeSwitches::new(RuntimeSwitchRole::Worker, AuditFailureMode::Shadow, true)
                .with_disposition_read_mode_baseline(DispositionReadMode::Classified),
        )
        .snapshot(Utc::now(), OperationsAlertCollectionOverdueRead::Skip)
        .await
        .expect("读取告警快照");
    assert_eq!(snapshot.delivery.expect("投递水位").failed, 0);
    // funding 预览不把这一从未发送的终态当成资产不明的失败投递。
    let preview = store.preview_funding("7654321".to_owned()).await.expect("读取 funding 预览");
    let delivery = sale.payment_delivery.to_string();
    assert!(
        !preview.blockers.iter().any(|blocker| blocker.contains(&delivery)),
        "{:?}",
        preview.blockers
    );
    assert_eq!(
        fulfillment(pool, sale.payment).await,
        ("failed".to_owned(), Some("recovery_reclaimed".to_owned()))
    );

    pool.close().await;
}

/// Worker 达到上限自动退回卖家时，同样终结被接管的待发原付款投递。
#[tokio::test]
async fn postgres_auto_return_to_seller_retires_the_requeued_original() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = sale(&store, 7_910_004).await;
    let at = created_at(pool, sale.recovery_id).await;
    requeued_before_return(pool, &sale, at).await;

    let advance = store
        .advance_due_cod_return(Utc::now())
        .await
        .expect("推进 COD 退回")
        .expect("到期记录必须推进");
    assert_eq!(advance.resolution.as_deref(), Some("returned_after_three_cod_returns"));
    assert_superseded(pool, &sale, "system").await;

    pool.close().await;
}

/// 反例：原付款投递已进入发送围栏（`sending`）时外部写入未解决，确认仍拒绝、什么都不改。
#[tokio::test]
async fn postgres_sending_original_still_blocks_confirmation() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = sale(&store, 7_910_005).await;
    let at = created_at(pool, sale.recovery_id).await;
    requeued_before_return(pool, &sale, at).await;
    sqlx::query(
        "UPDATE public.deliveries SET status = 'sending', claimed_at = $2, sending_at = $2,
             lease_token = gen_random_uuid(), lease_expires_at = now() + interval '5 minutes'
         WHERE id = $1",
    )
    .bind(sale.payment_delivery)
    .bind(at + Duration::minutes(40))
    .execute(pool)
    .await
    .expect("原付款投递进入发送");

    let error = store
        .confirm_recovery(confirmation(&sale, RecoveryConfirmationAction::Reclaimed))
        .await
        .expect_err("发送中的原投递不能收口");
    assert!(
        matches!(error, ApplicationError::RecoveryCommand(RecoveryCommandError::NotConfirmable)),
        "{error:?}"
    );
    assert_eq!(delivery_state(pool, sale.payment_delivery).await.0, "sending");
    assert!(dispositions(pool, sale.payment_delivery).await.is_empty());

    pool.close().await;
}

/// 原付款投递已终结为 `failed/recovery_superseded`：fence 推进一次、结果明确未外写，终结后的
/// fence 上恰有一条 `cod_returned` 处置，并有同一操作者来源的 `delivery.fail` 审计。
async fn assert_superseded(pool: &PgPool, sale: &Sale, source: &str) {
    assert_eq!(
        delivery_state(pool, sale.payment_delivery).await,
        ("failed".to_owned(), Some("recovery_superseded".to_owned()), 4, true)
    );
    assert_eq!(
        disposition_fences(pool, sale.payment_delivery).await,
        vec![("cod_returned".to_owned(), 4)]
    );
    let audits: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT source, metadata FROM public.audit_records
         WHERE entity_type = 'delivery' AND entity_id = $1 AND action = 'delivery.fail'",
    )
    .bind(sale.payment_delivery.to_string())
    .fetch_all(pool)
    .await
    .expect("读取投递审计");
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].0, source);
    assert_eq!(audits[0].1["failure_code"], "recovery_superseded");
    assert_eq!(audits[0].1["settlement_recovery_id"], sale.recovery_id.to_string());
}
