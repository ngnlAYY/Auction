//! 顺序消费所有已接受竞价事实；代理创建与网站普通出价都在同一事务内自动标记，
//! 慢速代理仍可反击时只记待定被超提醒（W14.11），能行动时才入队。

use auction_domain::{AcceptedBid, LotLeadership, ProxyAuthorization, UserId};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::*;
use crate::database::mail_delivery::MailDeliveryTransactionError;

pub(crate) async fn auto_watch_proxy(
    transaction: &mut Transaction<'_, Postgres>,
    state: &ProxyAuthorization,
) -> Result<(), CompetitionTransactionError> {
    // 新授权是新的用户意图，即使已经标记也推进 fence；自动追价和降额不进入此处。
    auto_watch(
        transaction,
        state.user_id.as_uuid(),
        state.lot_id.as_uuid(),
        "lot_watch.proxy_created",
    )
    .await
}

/// W14.5：网站普通出价在同一事务内自动标记，与新建代理一致。
/// 被超提醒只服务后续领先周期；重复出价推进版本但不会凭空产生通知。
pub(crate) async fn auto_watch_bid(
    transaction: &mut Transaction<'_, Postgres>,
    bid: &AcceptedBid,
) -> Result<(), CompetitionTransactionError> {
    if bid.mechanism != auction_domain::BidMechanism::Ordinary {
        return Ok(());
    }
    auto_watch(transaction, bid.bidder_id.as_uuid(), bid.lot_id.as_uuid(), "lot_watch.bid_created")
        .await
}

async fn auto_watch(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    lot_id: Uuid,
    action: &str,
) -> Result<(), CompetitionTransactionError> {
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO public.lot_watches
        (user_id, lot_id, watched, channel, version) VALUES ($1, $2, true, 'mm', 1)
        ON CONFLICT (user_id, lot_id) DO UPDATE SET watched = true,
        version = lot_watches.version + 1, updated_at = clock_timestamp() RETURNING version",
    )
    .bind(user_id)
    .bind(lot_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(sql_error)?;
    audit(transaction, user_id, lot_id, action, serde_json::json!({"version": version})).await
}

pub(crate) async fn apply_bid(
    transaction: &mut Transaction<'_, Postgres>,
    bid: &AcceptedBid,
    occurred_at: DateTime<Utc>,
    notifications: &dyn auction_application::NotificationRecorder,
    public_base_url: Option<&str>,
) -> Result<(), CompetitionTransactionError> {
    let lot_id = bid.lot_id.as_uuid();
    let row = match lock_leadership(transaction, lot_id).await? {
        Some(row) => row,
        None => {
            // 迁移之后创建的 lot 懒初始化；当前 lot 尚未写入本事务最终快照。只有首笔竞价多一次
            // 往返，并发初始化由唯一键与随后的行锁串行化。
            sqlx::query(
                "INSERT INTO public.lot_watch_leadership (lot_id, leader_user_id, cycle)
                SELECT id, leader_user_id, state_version FROM public.lots WHERE id = $1
                ON CONFLICT (lot_id) DO NOTHING",
            )
            .bind(lot_id)
            .execute(&mut **transaction)
            .await
            .map_err(sql_error)?;
            lock_leadership(transaction, lot_id).await?.ok_or_else(corrupt)?
        }
    };
    let leader: Option<Uuid> = row.try_get("leader_user_id").map_err(sql_error)?;
    let cycle: i64 = row.try_get("cycle").map_err(sql_error)?;
    let mut leadership = LotLeadership {
        leader: leader.map(UserId::from_uuid),
        cycle: u64::try_from(cycle).map_err(|error| {
            let _ = error;
            corrupt()
        })?,
    };
    if leadership.leader == Some(bid.bidder_id) {
        return Ok(());
    }
    let displaced = leadership.accept(bid.bidder_id, bid.state_version);
    invalidate(transaction, bid.bidder_id.as_uuid(), lot_id, "leading_again").await?;
    sqlx::query(
        "UPDATE public.lot_watch_leadership SET leader_user_id = $2, cycle = $3 WHERE lot_id = $1",
    )
    .bind(lot_id)
    .bind(bid.bidder_id.as_uuid())
    .bind(i64::try_from(leadership.cycle).map_err(|error| {
        let _ = error;
        corrupt()
    })?)
    .execute(&mut **transaction)
    .await
    .map_err(sql_error)?;
    if let Some((user_id, cycle)) = displaced {
        notify(
            transaction,
            bid,
            user_id.as_uuid(),
            cycle,
            occurred_at,
            notifications,
            public_base_url,
        )
        .await?;
    }
    // 代理状态在本事务内已更新：慢速代理耗尽或被更高普通出价击败后，
    // 待定提醒在此刻入队；反超成功则退役待定事实。
    resolve_pending(transaction, lot_id, occurred_at, notifications, public_base_url).await?;
    Ok(())
}

async fn lock_leadership(
    transaction: &mut Transaction<'_, Postgres>,
    lot_id: Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, CompetitionTransactionError> {
    sqlx::query(
        "SELECT leader_user_id, cycle FROM public.lot_watch_leadership
        WHERE lot_id = $1 FOR UPDATE",
    )
    .bind(lot_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(sql_error)
}

/// 通知所需的全部公开字段与当前提醒渠道；同一事务内只读一次。
struct OutbidTarget {
    watch_version: i64,
    channel: String,
    preference_version: i64,
    username: String,
    world_id: String,
    forum_account_id: Option<String>,
    lot_code: String,
    item_name: String,
    deadline: Option<DateTime<Utc>>,
    auction_status: String,
    slow_active: bool,
}

fn target_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<OutbidTarget, CompetitionTransactionError> {
    let category: String = row.try_get("category").map_err(sql_error)?;
    let lot_number: i32 = row.try_get("lot_number").map_err(sql_error)?;
    let lot_number = u32::try_from(lot_number).map_err(|error| {
        let _ = error;
        corrupt()
    })?;
    Ok(OutbidTarget {
        watch_version: row.try_get("version").map_err(sql_error)?,
        channel: row.try_get("lot_watch_channel").map_err(sql_error)?,
        preference_version: row.try_get("lot_watch_channel_version").map_err(sql_error)?,
        username: row.try_get("username").map_err(sql_error)?,
        world_id: row.try_get("world_id").map_err(sql_error)?,
        forum_account_id: row.try_get("forum_account_id").map_err(sql_error)?,
        lot_code: auction_application::lot_code(&category, lot_number),
        item_name: row.try_get("equipment_name").map_err(sql_error)?,
        deadline: row.try_get("effective_end_at").map_err(sql_error)?,
        auction_status: row.try_get("auction_status").map_err(sql_error)?,
        slow_active: row.try_get("slow_active").map_err(sql_error)?,
    })
}

async fn load_target(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    lot_id: Uuid,
) -> Result<Option<OutbidTarget>, CompetitionTransactionError> {
    let row = sqlx::query(
        "SELECT watch.version, u.username, a.world_id,
                a.status AS auction_status, u.lot_watch_channel,
                u.lot_watch_channel_version, u.forum_account_id,
                lot.lot_number, e.category, e.name AS equipment_name,
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

async fn notify(
    transaction: &mut Transaction<'_, Postgres>,
    bid: &AcceptedBid,
    user_id: Uuid,
    cycle: u64,
    occurred_at: DateTime<Utc>,
    notifications: &dyn auction_application::NotificationRecorder,
    public_base_url: Option<&str>,
) -> Result<(), CompetitionTransactionError> {
    let lot_id = bid.lot_id.as_uuid();
    let Some(target) = load_target(transaction, user_id, lot_id).await? else {
        return Ok(());
    };
    let amount = bid.amount.value();
    // 决策 67：慢速代理仍会自动反击时，持有人还不能行动；只记待定事实。
    if slow_proxy_can_act(&target, occurred_at) {
        defer(transaction, bid, user_id, cycle, amount, &target, occurred_at).await?;
        return Ok(());
    }
    enqueue(
        transaction,
        bid.id.as_uuid(),
        user_id,
        lot_id,
        cycle,
        amount,
        &target,
        occurred_at,
        notifications,
        public_base_url,
    )
    .await
}

fn slow_proxy_can_act(target: &OutbidTarget, occurred_at: DateTime<Utc>) -> bool {
    target.slow_active
        && target.auction_status == "live"
        && target.deadline.is_some_and(|deadline| deadline > occurred_at)
}

fn notice<'a>(
    target: &'a OutbidTarget,
    public_base_url: Option<&'a str>,
) -> auction_application::OutbidNotice<'a> {
    auction_application::OutbidNotice {
        lot_code: &target.lot_code,
        item_name: &target.item_name,
        deadline: target.deadline,
        site_url: public_base_url,
    }
}

async fn defer(
    transaction: &mut Transaction<'_, Postgres>,
    bid: &AcceptedBid,
    user_id: Uuid,
    cycle: u64,
    amount: i64,
    target: &OutbidTarget,
    occurred_at: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    let lot_id = bid.lot_id.as_uuid();
    let preference_channel = match target.channel.as_str() {
        "pm" | "mm" => target.channel.as_str(),
        _ => return Err(corrupt()),
    };
    if target.preference_version <= 0 {
        return Err(corrupt());
    }
    let id = pending_id(lot_id, user_id, cycle);
    sqlx::query(
        "INSERT INTO public.lot_watch_pending_notifications
        (id, user_id, lot_id, leadership_cycle, displaced_bid_id, watch_version,
         preference_version, channel, recipient, amount, created_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
        ON CONFLICT (lot_id, user_id, leadership_cycle) DO NOTHING",
    )
    .bind(id)
    .bind(user_id)
    .bind(lot_id)
    .bind(i64::try_from(cycle).map_err(|error| {
        let _ = error;
        corrupt()
    })?)
    .bind(bid.id.as_uuid())
    .bind(target.watch_version)
    .bind(target.preference_version)
    .bind(preference_channel)
    .bind(recipient_for(target))
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

fn recipient_for(target: &OutbidTarget) -> Option<String> {
    (target.channel == "pm").then(|| target.forum_account_id.clone()).flatten()
}

// 事务装配按显式列/身份/通知依赖逐项传递，避免为单次调用引入临时容器。
#[allow(clippy::too_many_arguments)]
async fn enqueue(
    transaction: &mut Transaction<'_, Postgres>,
    bid_id: Uuid,
    user_id: Uuid,
    lot_id: Uuid,
    cycle: u64,
    amount: i64,
    target: &OutbidTarget,
    occurred_at: DateTime<Utc>,
    notifications: &dyn auction_application::NotificationRecorder,
    public_base_url: Option<&str>,
) -> Result<(), CompetitionTransactionError> {
    match target.channel.as_str() {
        "mm" => {
            let command = auction_application::lot_watch_notification(
                lot_id,
                user_id,
                cycle,
                amount,
                notice(target, public_base_url),
                target.world_id.clone(),
                target.username.clone(),
            )
            .map_err(|error| {
                let _ = error;
                corrupt()
            })?;
            // 复用 delivery owner 的 SQL、身份冲突校验与审计，不自行复制 outbox 写入。
            let delivery = crate::database::mail_delivery::enqueue_in_transaction(
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
            .bind(i64::try_from(cycle).map_err(|error| { let _ = error; corrupt() })?)
            .bind(bid_id)
            .bind(target.watch_version)
            .bind(delivery.delivery_id)
            .bind(command.request_hash.as_bytes().as_slice())
            .bind(&command.recipient)
            .bind(crate::database::mail_delivery::payload(&command))
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
            .await
        }
        "pm" => {
            let notification = auction_application::lot_watch_pm_notification(
                lot_id,
                user_id,
                cycle,
                amount,
                notice(target, public_base_url),
                target.forum_account_id.clone(),
            )
            .map_err(|error| {
                let _ = error;
                corrupt()
            })?;
            sqlx::query(
                "INSERT INTO public.lot_watch_notifications
                (id,user_id,lot_id,leadership_cycle,displaced_bid_id,watch_version,command_id,
                 request_digest,recipient,payload,created_at,channel,preference_version)
                VALUES ($1,$2,$3,$4,$5,$6,$1,$7,$8,$9,$10,'pm',$11)",
            )
            .bind(notification.notification_id)
            .bind(user_id)
            .bind(lot_id)
            .bind(i64::try_from(cycle).map_err(|error| {
                let _ = error;
                corrupt()
            })?)
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
            .await
        }
        _ => Err(corrupt()),
    }
}

fn pending_id(lot_id: Uuid, user_id: Uuid, cycle: u64) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/lot-watch-pending-v1/{lot_id}/{user_id}/{cycle}").as_bytes(),
    )
}

/// 解析本 lot 的待定提醒：反超成功或 lot 结束即退役，代理耗尽即入队。
async fn resolve_pending(
    transaction: &mut Transaction<'_, Postgres>,
    lot_id: Uuid,
    occurred_at: DateTime<Utc>,
    notifications: &dyn auction_application::NotificationRecorder,
    public_base_url: Option<&str>,
) -> Result<(), CompetitionTransactionError> {
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
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(sql_error)?;
        let user_id: Uuid = row.try_get("user_id").map_err(sql_error)?;
        let cycle: i64 = row.try_get("leadership_cycle").map_err(sql_error)?;
        let cycle = u64::try_from(cycle).map_err(|error| {
            let _ = error;
            corrupt()
        })?;
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
        let closed =
            deadline.is_none_or(|deadline| deadline <= occurred_at) || auction_status != "live";
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
        let target = OutbidTarget {
            watch_version,
            preference_version,
            channel,
            forum_account_id: recipient.or(target.forum_account_id),
            ..target
        };
        enqueue(
            transaction,
            bid_id,
            user_id,
            lot_id,
            cycle,
            amount,
            &target,
            occurred_at,
            notifications,
            public_base_url,
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
    Ok(())
}

// 事务装配按显式列/身份/审计字段逐项传递，避免为单次调用引入临时容器。
#[allow(clippy::too_many_arguments)]
async fn resolve(
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

fn corrupt() -> CompetitionTransactionError {
    CompetitionTransactionError::Failed("装备标记通知事实无效".to_owned())
}
