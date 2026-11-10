//! Operations alert 各 owner 水位的安全读取。

use auction_application::core::ApplicationError;
use auction_application::operations::OperationsAlertSnapshot;
use chrono::{DateTime, Utc};

pub(crate) async fn snapshot_postgres(
    store: &super::super::DatabaseStore,
    now: DateTime<Utc>,
) -> Result<OperationsAlertSnapshot, ApplicationError> {
    // 每个 source 独立读取；单个 owner 的查询失败只生成该 source 的 unavailable signal，
    // 不让一个损坏的履约/恢复投影掩盖 process 或 delivery 的告警。
    let heartbeat_store = store.clone();
    let process = process_snapshot(&heartbeat_store).await.ok();
    let delivery = delivery_snapshot(store.pool(), now).await.ok();
    let fulfillment = fulfillment_snapshot(store.pool(), now).await.ok();
    let recovery = recovery_snapshot(store.pool(), now).await.ok();
    let mail = mail_snapshot(store.pool(), now).await.ok();
    let credential = credential_snapshot(store.pool()).await.ok();
    let lifecycle = lifecycle_snapshot(store.pool(), now).await.ok();
    let forum = forum_snapshot(store.pool(), now).await.ok();
    let notification = notification_snapshot(store.pool(), now).await.ok();
    let reconciliation = reconciliation_snapshot(store, now).await.ok();
    Ok(OperationsAlertSnapshot {
        process,
        delivery,
        fulfillment,
        recovery,
        mail,
        credential,
        lifecycle,
        forum,
        notification,
        reconciliation,
    })
}

mod accounts;
mod operations;
mod settlement;

use accounts::*;
use operations::*;
use settlement::*;

/// 已处置终态的统一判据：当前 fence 上已有解除异常终态的追加处置事实。
///
/// `delivery_dispositions.resolves_alert` 在写入时按领域闭集固定，交付与通知两个来源共用
/// 同一事实，读侧不再按来源拼接专用排除谓词（Admin 判失败、装备损失、历史自寄、
/// 预检退役与操作者确认各自只写一行处置）。预检退役中当前只有失去资格的关注提醒解除
/// 失败计数，沿用迁移 `000088` 之前的口径；放开其余退役属有意改变判定，须用户批准。
const UNDISPOSED: &str = "NOT EXISTS (
                    SELECT 1 FROM public.delivery_dispositions disposition
                    WHERE disposition.delivery_id = d.id
                      AND disposition.fencing_version = d.fencing_version
                      AND disposition.resolves_alert
                )";

fn age_seconds(now: DateTime<Utc>, observed_at: Option<DateTime<Utc>>) -> i64 {
    observed_at.map(|value| now.signed_duration_since(value).num_seconds().max(0)).unwrap_or(0)
}
