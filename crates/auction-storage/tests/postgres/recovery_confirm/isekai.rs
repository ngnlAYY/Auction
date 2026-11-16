//! isekai 载体 COD 被退回、载体收回证明成立后，Admin 可直接取消。退回次数未达冻结上限时
//! 不要求拉黑，达到上限仍要求既定拉黑完成；确认按钮、待办计数与确认事务共用同一规则。

use auction_application::core::ApplicationError;
use auction_application::operations::OperationsAlertConsole;
use auction_application::recovery::{
    RecoveryCommandError, RecoveryCommandRepository, RecoveryConfirmationAction,
    RecoveryReportReader,
};
use auction_domain::AuctionId;
use auction_storage::DatabaseStore;

use super::fixture::{Sale, SaleSpec, insert_reclaiming_sale};
use super::{confirmation, dispositions, fulfillment};
use crate::support;

async fn allowed_commands(store: &DatabaseStore, sale: &Sale) -> Vec<String> {
    let report = store
        .recovery_report(AuctionId::from_uuid(sale.auction_id))
        .await
        .expect("读取恢复报告")
        .expect("场次存在");
    report
        .recoveries
        .into_iter()
        .find(|entry| entry.id.as_uuid() == sale.recovery_id)
        .expect("报告包含该恢复记录")
        .allowed_commands
}

fn not_confirmable(error: &ApplicationError) -> bool {
    matches!(error, ApplicationError::RecoveryCommand(RecoveryCommandError::NotConfirmable))
}

async fn isekai_sale(
    store: &DatabaseStore,
    sequence: i64,
    count: i32,
    limit: i32,
    blacklist: bool,
) -> Sale {
    insert_reclaiming_sale(
        store.postgres_pool(),
        SaleSpec {
            world: "isekai",
            sequence,
            cod_return_count: count,
            cod_return_limit: limit,
            blacklist_satisfied: blacklist,
        },
    )
    .await
}

#[tokio::test]
async fn postgres_isekai_cancel_below_the_limit_needs_no_blacklist() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = isekai_sale(&store, 7_700_001, 1, 3, false).await;

    assert_eq!(allowed_commands(&store, &sale).await, vec!["cancelled".to_owned()]);
    assert_eq!(store.pending_counts().await.expect("待办计数").recovery_confirmation, 1);

    let result = store
        .confirm_recovery(confirmation(&sale, RecoveryConfirmationAction::Cancelled))
        .await
        .expect("未达上限的载体收回可直接取消");
    assert_eq!(result.resolution, "cancelled");
    assert_eq!(
        fulfillment(pool, sale.payment).await,
        ("failed".to_owned(), Some("recovery_cancelled".to_owned()))
    );
    for dependent in [sale.handoff.expect("isekai 交付义务"), sale.seller] {
        assert_eq!(
            fulfillment(pool, dependent).await,
            ("failed".to_owned(), Some("recovery_sale_voided".to_owned()))
        );
    }
    assert_eq!(fulfillment(pool, sale.refund).await, ("pending".to_owned(), None));
    assert_eq!(
        dispositions(pool, sale.payment_delivery).await,
        vec![(
            "cod_returned".to_owned(),
            Some("recovery_cancelled".to_owned()),
            "admin".to_owned()
        )]
    );
    // 取消不改变冻结的处罚阈值：不建立拉黑动作。
    let blacklist: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_recovery_actions
         WHERE recovery_id = $1 AND kind = 'buyer_blacklist'",
    )
    .bind(sale.recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取拉黑动作");
    assert_eq!(blacklist, 0);
    assert_eq!(store.pending_counts().await.expect("待办计数").recovery_confirmation, 0);

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_isekai_cancel_at_the_limit_still_requires_the_blacklist() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    // 冻结上限 1 与 2 时，达到上限且拉黑未完成：按钮不出现、待办不计、确认拒绝。
    for (sequence, count, limit) in [(7_700_011, 1, 1), (7_700_012, 2, 2)] {
        let sale = isekai_sale(&store, sequence, count, limit, false).await;
        assert!(allowed_commands(&store, &sale).await.is_empty(), "limit={limit}");
        let error = store
            .confirm_recovery(confirmation(&sale, RecoveryConfirmationAction::Cancelled))
            .await
            .expect_err("达到上限必须先完成拉黑");
        assert!(not_confirmable(&error), "{error:?}");
        assert_eq!(fulfillment(store.postgres_pool(), sale.payment).await.0, "enqueued");
    }
    assert_eq!(store.pending_counts().await.expect("待办计数").recovery_confirmation, 0);

    // 拉黑已完成时按既定规则可确认。
    let sale = isekai_sale(&store, 7_700_013, 2, 2, true).await;
    assert_eq!(allowed_commands(&store, &sale).await, vec!["cancelled".to_owned()]);
    store
        .confirm_recovery(confirmation(&sale, RecoveryConfirmationAction::Cancelled))
        .await
        .expect("拉黑完成后可确认取消");

    store.postgres_pool().close().await;
}

/// 规则只放宽 isekai 取消：main 收回未达上限时仍由系统重寄、不能确认；已付款的载体不能取消；
/// 过期版本按 StaleState 拒绝。
#[tokio::test]
async fn postgres_confirmation_rules_outside_the_isekai_relaxation_are_unchanged() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    let main = insert_reclaiming_sale(
        pool,
        SaleSpec {
            world: "main",
            sequence: 7_700_021,
            cod_return_count: 1,
            cod_return_limit: 3,
            blacklist_satisfied: false,
        },
    )
    .await;
    assert!(allowed_commands(&store, &main).await.is_empty());
    let error = store
        .confirm_recovery(confirmation(&main, RecoveryConfirmationAction::Reclaimed))
        .await
        .expect_err("main 收回始终要求拉黑");
    assert!(not_confirmable(&error), "{error:?}");

    let paid = isekai_sale(&store, 7_700_022, 1, 3, false).await;
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'satisfied', proof_kind = 'mooglemail_sent_collected',
             proof_reference = '1234567', proof_digest = decode(repeat('e1', 32), 'hex'),
             proof_observed_at = clock_timestamp(), completed_at = clock_timestamp()
         WHERE obligation_id = $1",
    )
    .bind(paid.payment)
    .execute(pool)
    .await
    .expect("构造已付款载体");
    let error = store
        .confirm_recovery(confirmation(&paid, RecoveryConfirmationAction::Cancelled))
        .await
        .expect_err("已付款的载体不能取消");
    assert!(not_confirmable(&error), "{error:?}");

    let stale = isekai_sale(&store, 7_700_023, 1, 3, false).await;
    let mut command = confirmation(&stale, RecoveryConfirmationAction::Cancelled);
    command.expected_state_version -= 1;
    command.request_hash =
        auction_application::recovery::RecoveryConfirmationCommand::legacy_request_hash(
            stale.recovery_id,
            command.expected_state_version,
            RecoveryConfirmationAction::Cancelled,
            &command.reason,
        )
        .expect("确认请求摘要");
    let error = store.confirm_recovery(command).await.expect_err("过期版本必须拒绝");
    assert!(
        matches!(error, ApplicationError::RecoveryCommand(RecoveryCommandError::StaleState)),
        "{error:?}"
    );
    assert_eq!(fulfillment(pool, stale.payment).await.0, "enqueued");

    store.postgres_pool().close().await;
}
