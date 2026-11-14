//! 待定被超提醒的结算：反超成功或 lot 结束即退役，代理耗尽即入队。

use auction_application::admin::AdminRuntimeMailTemplatesConfig;
use auction_application::core::CompetitionTransactionError;
use auction_domain::LotWatchChannel;
use chrono::{DateTime, Utc};
use sqlx::Postgres;
use sqlx::Row;
use uuid::Uuid;

use super::outbid::{OutbidTarget, enqueue, load_target};
use super::{corrupt, corrupt_value};
use crate::database::TrackedTransaction as Transaction;
use crate::database::lot_watch::persistence::{audit, sql_error};

/// 解析本 lot 的待定提醒：反超成功或 lot 结束即退役，代理耗尽即入队。
/// 返回本轮是否入队了至少一条新的 `lot_watch` 投递。
pub(crate) async fn resolve_pending(
    transaction: &mut Transaction<'_, Postgres>,
    lot_id: Uuid,
    occurred_at: DateTime<Utc>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
    public_base_url: Option<&str>,
    mail_templates: &AdminRuntimeMailTemplatesConfig,
) -> Result<bool, CompetitionTransactionError> {
    let rows = sqlx::query(
        "SELECT pending.id, pending.user_id, pending.leadership_cycle, pending.displaced_bid_id,
                pending.amount, pending.channel, pending.recipient, pending.watch_version,
                pending.preference_version,
                lot.leader_user_id, lot.effective_end_at, auction.status AS auction_status,
                EXISTS (SELECT 1 FROM public.bid_proxy_authorizations proxy
                        WHERE proxy.lot_id = pending.lot_id AND proxy.user_id = pending.user_id
                          AND proxy.mode = 'slow' AND proxy.status = 'active') AS slow_active
         FROM public.lot_watch_pending_notifications pending
         JOIN public.lots lot ON lot.id = pending.lot_id
         JOIN public.auctions auction ON auction.id = lot.auction_id
         WHERE pending.lot_id = $1 AND pending.status = 'pending'
         ORDER BY pending.created_at, pending.id
         FOR UPDATE OF pending",
    )
    .bind(lot_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(sql_error)?;
    let mut wakeup = false;
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(sql_error)?;
        let user_id: Uuid = row.try_get("user_id").map_err(sql_error)?;
        let cycle: i64 = row.try_get("leadership_cycle").map_err(sql_error)?;
        let cycle = u64::try_from(cycle).map_err(corrupt_value)?;
        let bid_id: Uuid = row.try_get("displaced_bid_id").map_err(sql_error)?;
        let amount: i64 = row.try_get("amount").map_err(sql_error)?;
        let channel: String = row.try_get("channel").map_err(sql_error)?;
        let recipient: Option<String> = row.try_get("recipient").map_err(sql_error)?;
        let watch_version: i64 = row.try_get("watch_version").map_err(sql_error)?;
        let preference_version: i64 = row.try_get("preference_version").map_err(sql_error)?;
        let leader: Option<Uuid> = row.try_get("leader_user_id").map_err(sql_error)?;
        let deadline: Option<DateTime<Utc>> = row.try_get("effective_end_at").map_err(sql_error)?;
        let auction_status: String = row.try_get("auction_status").map_err(sql_error)?;
        let slow_active: bool = row.try_get("slow_active").map_err(sql_error)?;

        if leader == Some(user_id) {
            resolve(
                transaction,
                id,
                user_id,
                lot_id,
                cycle,
                "retired",
                "leading_again",
                occurred_at,
            )
            .await?;
            continue;
        }
        // Final 仍允许慢代理被提升后行动；只有 lot 截止或场次离开 live/final 才关闭，
        // 否则 Final 中代理耗尽时会把本应入队的提醒提前退役。
        let closed = deadline.is_none_or(|deadline| deadline <= occurred_at)
            || !matches!(auction_status.as_str(), "live" | "final");
        if closed {
            resolve(transaction, id, user_id, lot_id, cycle, "retired", "lot_closed", occurred_at)
                .await?;
            continue;
        }
        if slow_active {
            // 持有人仍可能在等待反击，继续待定；不重复写事实。
            continue;
        }
        let Some(target) = load_target(transaction, user_id, lot_id).await? else {
            resolve(
                transaction,
                id,
                user_id,
                lot_id,
                cycle,
                "retired",
                "watch_removed",
                occurred_at,
            )
            .await?;
            continue;
        };
        // 使用入队时观察到的冻结渠道/收件人快照；当前偏好只影响未来通知。
        // 待定事实每行只属于一个渠道；闭集之外的渠道名按事实损坏处理。
        let channel = LotWatchChannel::parse(&channel).ok_or_else(corrupt)?;
        // MM 的 recipient 是冻结用户名；旧行可能为空，此时保留当前名称并在发送前检查。
        // PM 的 recipient 是冻结的论坛账号。
        let (username, forum_account_id) = match channel {
            LotWatchChannel::Mm => (recipient.unwrap_or(target.username), target.forum_account_id),
            LotWatchChannel::Pm => (target.username, recipient.or(target.forum_account_id)),
        };
        let target = OutbidTarget {
            watch_version,
            preference_version,
            username,
            forum_account_id,
            ..target
        };
        wakeup |= enqueue(
            transaction,
            bid_id,
            user_id,
            lot_id,
            cycle,
            amount,
            &target,
            channel,
            occurred_at,
            notifications,
            public_base_url,
            mail_templates,
        )
        .await?;
        resolve(
            transaction,
            id,
            user_id,
            lot_id,
            cycle,
            "enqueued",
            "slow_proxy_exhausted",
            occurred_at,
        )
        .await?;
    }
    Ok(wakeup)
}

// 事务装配按显式列/身份/审计字段逐项传递，避免为单次调用引入临时容器。
#[allow(clippy::too_many_arguments)]
pub(super) async fn resolve(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    user_id: Uuid,
    lot_id: Uuid,
    cycle: u64,
    status: &str,
    reason: &str,
    occurred_at: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    let updated = sqlx::query(
        "UPDATE public.lot_watch_pending_notifications
         SET status = $2, resolution_reason = $3, resolved_at = $4
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .bind(status)
    .bind(reason)
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(sql_error)?;
    if updated.rows_affected() != 1 {
        return Err(corrupt());
    }
    audit(
        transaction,
        user_id,
        lot_id,
        "lot_watch.pending_resolved",
        serde_json::json!({"cycle": cycle, "status": status, "reason": reason}),
    )
    .await
}
