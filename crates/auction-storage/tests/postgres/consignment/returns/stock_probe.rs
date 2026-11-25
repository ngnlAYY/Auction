//! 寄售存量探针的恢复待办（PIWS3b，PIWW2 审计问题 3）：挂单已终态、只剩未处置的退件消费或到期未提醒的
//! 已售合同时，探针仍为真（关闭寄售时 Worker 仍须收口）。
//!
//! 只测核心规则：安全边界（存量判定失败关闭，不把恢复待办当作空库）。

use std::sync::Arc;

use auction_application::consignment::{PaymentReminderService, PaymentReminderStore};

use super::super::contracts::fixture::CONTRACT;
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{original_dispatch_sql, seed_purchased};
use super::super::ledger136::purchase::buyer_claimed_original_sql;
use super::fixture::{Rts, seed_consumed};
use crate::support::TestDatabase;

/// 让寄售处置投递全部以失败收口（处置工作这一项不再是存量）。
const CLOSE_DISPOSITION_DELIVERIES: &str = "UPDATE public.deliveries
     SET status = 'failed', completed_at = clock_timestamp(), failure_code = 'fake_closed_work',
         lease_token = NULL, lease_expires_at = NULL
     WHERE id IN (SELECT delivery_id FROM public.mail_disposition_work)
       AND status NOT IN ('succeeded', 'failed', 'manual_reconcile');";

/// 已消费的现金退款腿尚未处置，挂单、position 与处置投递都已终结、提醒已作决定：探针为真。
///
/// 挂单与 position 经真实写序终结（原件腿首派后买家确领原件）；该合同的到期提醒由提醒写端作出唯一决定。
#[tokio::test]
async fn postgres_consignment_lookup_probe_counts_unresolved_return_consumption() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_consumed(pool, &store, Rts { n: 1 }).await;
    succeeds(pool, &(original_dispatch_sql() + &buyer_claimed_original_sql())).await;
    succeeds(pool, CLOSE_DISPOSITION_DELIVERIES).await;
    let reminders: Arc<dyn PaymentReminderStore> = store.clone();
    PaymentReminderService::new(reminders)
        .remind(CONTRACT.parse().expect("合同 ID"))
        .await
        .expect("提醒决定");
    assert!(store.has_consignment_work().await.expect("探针"), "未处置的退件消费是存量");
}

/// 已售合同付款期限已过、首派已证寄出、尚无提醒，挂单与 position 已经真实写序终结：探针为真。
#[tokio::test]
async fn postgres_consignment_lookup_probe_counts_due_payment_reminder() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = database.store().await.expect("运行账号");
    let pool = owner.postgres_pool();
    seed_purchased(pool).await;
    succeeds(pool, &(original_dispatch_sql() + &buyer_claimed_original_sql())).await;
    succeeds(pool, CLOSE_DISPOSITION_DELIVERIES).await;
    assert!(store.has_consignment_work().await.expect("探针"), "到期未提醒的已售合同是存量");
}
