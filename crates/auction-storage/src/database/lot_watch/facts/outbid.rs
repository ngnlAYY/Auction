//! 被超提醒的目标装载、慢速代理延后与按渠道入队。

use auction_domain::{AcceptedBid, LotWatchChannel, LotWatchChannels};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::lot_watch::persistence::{audit, sql_error};
use crate::database::mail::delivery::MailDeliveryTransactionError;
use auction_application::admin::AdminRuntimeMailTemplatesConfig;
use auction_application::core::CompetitionTransactionError;
use sqlx::Postgres;

use super::{corrupt, corrupt_value};

/// 通知所需的全部公开字段与当前提醒渠道偏好；同一事务内只读一次。
pub(super) struct OutbidTarget {
    pub(super) watch_version: i64,
    /// 账号偏好：MM 与 PM 各自独立，可都启用或都关闭。
    pub(super) channels: LotWatchChannels,
    pub(super) preference_version: i64,
    pub(super) username: String,
    pub(super) world_id: String,
    pub(super) forum_account_id: Option<String>,
    pub(super) lot_code: String,
    pub(super) item_name: String,
    pub(super) deadline: Option<DateTime<Utc>>,
    pub(super) auction_status: String,
    pub(super) slow_active: bool,
}

pub(super) fn target_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<OutbidTarget, CompetitionTransactionError> {
    let category: String = row.try_get("category").map_err(sql_error)?;
    let lot_number: i32 = row.try_get("lot_number").map_err(sql_error)?;
    let lot_number = u32::try_from(lot_number).map_err(corrupt_value)?;
    let channels: String = row.try_get("lot_watch_channel").map_err(sql_error)?;
    Ok(OutbidTarget {
        watch_version: row.try_get("version").map_err(sql_error)?,
        // 闭集之外的偏好值按事实损坏处理，不猜测渠道。
        channels: LotWatchChannels::preference(&channels).ok_or_else(corrupt)?,
        preference_version: row.try_get("lot_watch_channel_version").map_err(sql_error)?,
        username: row.try_get("username").map_err(sql_error)?,
        world_id: row.try_get("world_id").map_err(sql_error)?,
        forum_account_id: row.try_get("forum_account_id").map_err(sql_error)?,
        lot_code: auction_application::public::lot_code(&category, lot_number),
        item_name: row.try_get("equipment_name").map_err(sql_error)?,
        deadline: row.try_get("effective_end_at").map_err(sql_error)?,
        auction_status: row.try_get("auction_status").map_err(sql_error)?,
        slow_active: row.try_get("slow_active").map_err(sql_error)?,
    })
}

pub(super) async fn load_target(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    lot_id: Uuid,
) -> Result<Option<OutbidTarget>, CompetitionTransactionError> {
    let row = sqlx::query(
        "SELECT watch.version, u.username, a.world_id,
                a.status AS auction_status, u.lot_watch_channel,
                u.lot_watch_channel_version, u.forum_account_id,
                -- 玩家显示的 lot code 使用类别内编号；lot_number 是场次全局序号。
                lot.category_number AS lot_number, e.category, e.name AS equipment_name,
                lot.effective_end_at,
                EXISTS (SELECT 1 FROM public.bid_proxy_authorizations proxy
                        WHERE proxy.lot_id = watch.lot_id AND proxy.user_id = watch.user_id
                          AND proxy.mode = 'slow' AND proxy.status = 'active') AS slow_active
         FROM public.lot_watches watch
         JOIN public.users u ON u.id = watch.user_id
         JOIN public.lots lot ON lot.id = watch.lot_id
         JOIN public.auctions a ON a.id = lot.auction_id
         JOIN public.equipment e ON e.id = lot.equipment_id
         WHERE watch.user_id = $1 AND watch.lot_id = $2 AND watch.watched",
    )
    .bind(user_id)
    .bind(lot_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(sql_error)?;
    row.as_ref().map(target_from_row).transpose()
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn notify(
    transaction: &mut Transaction<'_, Postgres>,
    bid: &AcceptedBid,
    user_id: Uuid,
    cycle: u64,
    occurred_at: DateTime<Utc>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
    public_base_url: Option<&str>,
    mail_templates: &AdminRuntimeMailTemplatesConfig,
) -> Result<bool, CompetitionTransactionError> {
    let lot_id = bid.lot_id.as_uuid();
    let Some(target) = load_target(transaction, user_id, lot_id).await? else {
        return Ok(false);
    };
    let amount = bid.amount.value();
    // 决策 67：慢速代理仍会自动反击时，持有人还不能行动；只记待定事实。
    let deferred = slow_proxy_can_act(&target, occurred_at);
    let mut wakeup = false;
    // MM 与 PM 各自独立：启用的每个渠道在本领先周期各建一条来源（或待定事实），
    // 每个渠道每周期最多一条；都未启用时标记只作收藏，不产生提醒。
    for channel in target.channels.enabled() {
        if deferred {
            defer(transaction, bid, user_id, cycle, amount, &target, channel, occurred_at).await?;
            continue;
        }
        wakeup |= enqueue(
            transaction,
            bid.id.as_uuid(),
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
    }
    Ok(wakeup)
}

pub(super) fn slow_proxy_can_act(target: &OutbidTarget, occurred_at: DateTime<Utc>) -> bool {
    target.slow_active
        && target.auction_status == "live"
        && target.deadline.is_some_and(|deadline| deadline > occurred_at)
}

pub(super) fn notice<'a>(
    target: &'a OutbidTarget,
    public_base_url: Option<&'a str>,
) -> auction_application::public::OutbidNotice<'a> {
    auction_application::public::OutbidNotice {
        lot_code: &target.lot_code,
        item_name: &target.item_name,
        deadline: target.deadline,
        site_url: public_base_url,
    }
}

// 待定事实按显式列与身份逐项传递，避免为单次调用引入临时容器。
#[allow(clippy::too_many_arguments)]
pub(super) async fn defer(
    transaction: &mut Transaction<'_, Postgres>,
    bid: &AcceptedBid,
    user_id: Uuid,
    cycle: u64,
    amount: i64,
    target: &OutbidTarget,
    channel: LotWatchChannel,
    occurred_at: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    let lot_id = bid.lot_id.as_uuid();
    let preference_channel = channel.as_str();
    if target.preference_version <= 0 {
        return Err(corrupt());
    }
    let id = pending_id(lot_id, user_id, cycle, channel);
    sqlx::query(
        "INSERT INTO public.lot_watch_pending_notifications
        (id, user_id, lot_id, leadership_cycle, displaced_bid_id, watch_version,
         preference_version, channel, recipient, amount, created_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
        ON CONFLICT (lot_id, user_id, leadership_cycle, channel) DO NOTHING",
    )
    .bind(id)
    .bind(user_id)
    .bind(lot_id)
    .bind(i64::try_from(cycle).map_err(corrupt_value)?)
    .bind(bid.id.as_uuid())
    .bind(target.watch_version)
    .bind(target.preference_version)
    .bind(preference_channel)
    .bind(recipient_for(target, channel))
    .bind(amount)
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(sql_error)?;
    audit(
        transaction,
        user_id,
        lot_id,
        "lot_watch.pending_created",
        serde_json::json!({"cycle": cycle, "channel": preference_channel}),
    )
    .await
}

pub(super) fn recipient_for(target: &OutbidTarget, channel: LotWatchChannel) -> Option<String> {
    // MM 冻结用户名，PM 冻结论坛账号；两者都在入队时快照，改名后由预检退役。
    match channel {
        LotWatchChannel::Mm => Some(target.username.clone()),
        LotWatchChannel::Pm => target.forum_account_id.clone(),
    }
}

// 事务装配按显式列/身份/通知依赖逐项传递，避免为单次调用引入临时容器。
#[allow(clippy::too_many_arguments)]
pub(super) async fn enqueue(
    transaction: &mut Transaction<'_, Postgres>,
    bid_id: Uuid,
    user_id: Uuid,
    lot_id: Uuid,
    cycle: u64,
    amount: i64,
    target: &OutbidTarget,
    channel: LotWatchChannel,
    occurred_at: DateTime<Utc>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
    public_base_url: Option<&str>,
    mail_templates: &AdminRuntimeMailTemplatesConfig,
) -> Result<bool, CompetitionTransactionError> {
    match channel {
        LotWatchChannel::Mm => {
            let command = auction_application::public::lot_watch_notification(
                lot_id,
                user_id,
                cycle,
                amount,
                notice(target, public_base_url),
                mail_templates,
                target.world_id.clone(),
                target.username.clone(),
            )
            .map_err(corrupt_value)?;
            // 复用 delivery owner 的 SQL、身份冲突校验与审计，不自行复制 outbox 写入。
            let delivery = crate::database::mail::delivery::enqueue_in_transaction(
                transaction,
                &command,
                None,
                Some(notifications),
            )
            .await
            .map_err(|error| match error {
                // 40001/40P01 必须返回竞价 runner，按原预算重试整个事务而非局部入队。
                MailDeliveryTransactionError::Sqlx(error) => sql_error(error),
                MailDeliveryTransactionError::Rejected(_) => corrupt(),
            })?;
            sqlx::query(
                "INSERT INTO public.lot_watch_notifications
                (id, user_id, lot_id, leadership_cycle, displaced_bid_id, watch_version, delivery_id,
                 command_id, request_digest, recipient, payload, created_at, preference_version)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$1,$8,$9,$10,$11,$12)",
            )
            .bind(command.command_id.as_uuid())
            .bind(user_id)
            .bind(lot_id)
            .bind(i64::try_from(cycle).map_err(corrupt_value)?)
            .bind(bid_id)
            .bind(target.watch_version)
            .bind(delivery.delivery_id)
            .bind(command.request_hash.as_bytes().as_slice())
            .bind(&command.recipient)
            .bind(crate::database::mail::delivery::payload(&command))
            .bind(occurred_at)
            .bind(target.preference_version)
            .execute(&mut **transaction)
            .await
            .map_err(sql_error)?;
            audit(
                transaction,
                user_id,
                lot_id,
                "lot_watch.outbid",
                serde_json::json!({"cycle": cycle, "bid_id": bid_id, "delivery_id": delivery.delivery_id}),
            )
            .await?;
            Ok(true)
        }
        LotWatchChannel::Pm => {
            let notification = auction_application::public::lot_watch_pm_notification(
                lot_id,
                user_id,
                cycle,
                amount,
                notice(target, public_base_url),
                mail_templates,
                target.forum_account_id.clone(),
            )
            .map_err(corrupt_value)?;
            sqlx::query(
                "INSERT INTO public.lot_watch_notifications
                (id,user_id,lot_id,leadership_cycle,displaced_bid_id,watch_version,command_id,
                 request_digest,recipient,payload,created_at,channel,preference_version)
                VALUES ($1,$2,$3,$4,$5,$6,$1,$7,$8,$9,$10,'pm',$11)",
            )
            .bind(notification.notification_id)
            .bind(user_id)
            .bind(lot_id)
            .bind(i64::try_from(cycle).map_err(corrupt_value)?)
            .bind(bid_id)
            .bind(target.watch_version)
            .bind(notification.digest.as_bytes().as_slice())
            .bind(&notification.recipient)
            .bind(&notification.payload)
            .bind(occurred_at)
            .bind(target.preference_version)
            .execute(&mut **transaction)
            .await
            .map_err(sql_error)?;
            sqlx::query(
                "INSERT INTO public.lot_watch_pm_deliveries
                (notification_id,status,failure_code,completed_at)
                VALUES ($1,$2,$3,CASE WHEN $2 = 'suppressed' THEN clock_timestamp() ELSE NULL END)",
            )
            .bind(notification.notification_id)
            .bind(if notification.recipient.is_some() { "pending" } else { "suppressed" })
            .bind(if notification.recipient.is_some() { None } else { Some("recipient_missing") })
            .execute(&mut **transaction)
            .await
            .map_err(sql_error)?;
            audit(
                transaction,
                user_id,
                lot_id,
                "lot_watch.outbid",
                serde_json::json!({
                    "notification_id": notification.notification_id,
                    "cycle": cycle,
                    "channel": "pm",
                    "suppressed": notification.recipient.is_none(),
                }),
            )
            .await?;
            Ok(false)
        }
    }
}

/// 待定事实身份：每个 lot × 用户 × 领先周期 × 渠道一条。MM 沿用引入渠道集合之前的身份，
/// PM 追加渠道后缀，账号同时启用两个渠道时两条待定事实互不冲突。
pub(super) fn pending_id(
    lot_id: Uuid,
    user_id: Uuid,
    cycle: u64,
    channel: LotWatchChannel,
) -> Uuid {
    let name = match channel {
        LotWatchChannel::Mm => format!("auction/lot-watch-pending-v1/{lot_id}/{user_id}/{cycle}"),
        LotWatchChannel::Pm => {
            format!("auction/lot-watch-pending-v1/{lot_id}/{user_id}/{cycle}/pm")
        }
    };
    Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes())
}
