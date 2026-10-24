//! 顺序消费所有已接受竞价事实；代理创建只在 Created 转换中自动标记。

use auction_domain::{AcceptedBid, LotLeadership, ProxyAuthorization, UserId};
use chrono::{DateTime, Utc};
use sqlx::Row;

use super::*;
use crate::database::mail_delivery::MailDeliveryTransactionError;

pub(crate) async fn auto_watch_proxy(
    transaction: &mut Transaction<'_, Postgres>,
    state: &ProxyAuthorization,
) -> Result<(), CompetitionTransactionError> {
    // 新授权是新的用户意图，即使已经标记也推进 fence；自动追价和降额不进入此处。
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO public.lot_watches
        (user_id, lot_id, watched, channel, version) VALUES ($1, $2, true, 'mm', 1)
        ON CONFLICT (user_id, lot_id) DO UPDATE SET watched = true,
        version = lot_watches.version + 1, updated_at = clock_timestamp() RETURNING version",
    )
    .bind(state.user_id.as_uuid())
    .bind(state.lot_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(sql_error)?;
    audit(
        transaction,
        state.user_id.as_uuid(),
        state.lot_id.as_uuid(),
        "lot_watch.proxy_created",
        serde_json::json!({"authorization_id": state.id, "version": version}),
    )
    .await
}

pub(crate) async fn apply_bid(
    transaction: &mut Transaction<'_, Postgres>,
    bid: &AcceptedBid,
    occurred_at: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    let lot_id = bid.lot_id.as_uuid();
    // 迁移之后创建的 lot 懒初始化；当前 lot 尚未写入本事务最终快照。
    sqlx::query(
        "INSERT INTO public.lot_watch_leadership (lot_id, leader_user_id, cycle)
        SELECT id, leader_user_id, state_version FROM public.lots WHERE id = $1
        ON CONFLICT (lot_id) DO NOTHING",
    )
    .bind(lot_id)
    .execute(&mut **transaction)
    .await
    .map_err(sql_error)?;
    let row = sqlx::query(
        "SELECT leader_user_id, cycle FROM public.lot_watch_leadership
        WHERE lot_id = $1 FOR UPDATE",
    )
    .bind(lot_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(sql_error)?;
    let leader: Option<Uuid> = row.try_get("leader_user_id").map_err(sql_error)?;
    let cycle: i64 = row.try_get("cycle").map_err(sql_error)?;
    let mut leadership = LotLeadership {
        leader: leader.map(UserId::from_uuid),
        cycle: u64::try_from(cycle).map_err(|_| corrupt())?,
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
    .bind(i64::try_from(leadership.cycle).map_err(|_| corrupt())?)
    .execute(&mut **transaction)
    .await
    .map_err(sql_error)?;
    if let Some((user_id, cycle)) = displaced {
        notify(transaction, bid, user_id.as_uuid(), cycle, occurred_at).await?;
    }
    Ok(())
}

async fn notify(
    transaction: &mut Transaction<'_, Postgres>,
    bid: &AcceptedBid,
    user_id: Uuid,
    cycle: u64,
    occurred_at: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    let lot_id = bid.lot_id.as_uuid();
    let row = sqlx::query("SELECT watch.version, u.username, a.world_id,
        u.lot_watch_channel, u.lot_watch_channel_version, u.forum_account_id
        FROM public.lot_watches watch JOIN public.users u ON u.id = watch.user_id
        JOIN public.lots lot ON lot.id = watch.lot_id JOIN public.auctions a ON a.id = lot.auction_id
        WHERE watch.user_id = $1 AND watch.lot_id = $2 AND watch.watched AND watch.channel = 'mm'")
        .bind(user_id).bind(lot_id).fetch_optional(&mut **transaction).await.map_err(sql_error)?;
    let Some(row) = row else {
        return Ok(());
    };
    let version: i64 = row.try_get("version").map_err(sql_error)?;
    let channel: String = row.try_get("lot_watch_channel").map_err(sql_error)?;
    let preference_version: i64 = row.try_get("lot_watch_channel_version").map_err(sql_error)?;
    // 只读取既有 Serializable 快照；不得在此无序追加旧 leader 的 user 锁。
    if channel == "pm" {
        return super::pm_fact::notify_pm(
            transaction,
            bid,
            super::pm_fact::PmFact {
                user_id,
                cycle,
                watch_version: version,
                preference_version,
                recipient: row.try_get("forum_account_id").map_err(sql_error)?,
                occurred_at,
            },
        )
        .await;
    }
    if channel != "mm" || preference_version <= 0 {
        return Err(corrupt());
    }
    let command = auction_application::lot_watch_notification(
        lot_id,
        user_id,
        cycle,
        bid.amount.value(),
        row.try_get("world_id").map_err(sql_error)?,
        row.try_get("username").map_err(sql_error)?,
    )
    .map_err(|_| corrupt())?;
    // 复用 delivery owner 的 SQL、身份冲突校验与审计，不自行复制 outbox 写入。
    let delivery =
        crate::database::mail_delivery::enqueue_in_transaction(transaction, &command, None)
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
    .bind(i64::try_from(cycle).map_err(|_| corrupt())?)
    .bind(bid.id.as_uuid())
    .bind(version)
    .bind(delivery.delivery_id)
    .bind(command.request_hash.as_bytes().as_slice())
    .bind(&command.recipient)
    .bind(crate::database::mail_delivery::payload(&command))
    .bind(occurred_at)
    .bind(preference_version)
    .execute(&mut **transaction)
    .await
    .map_err(sql_error)?;
    audit(
        transaction,
        user_id,
        lot_id,
        "lot_watch.outbid",
        serde_json::json!({"cycle": cycle, "bid_id": bid.id, "delivery_id": delivery.delivery_id}),
    )
    .await
}

fn corrupt() -> CompetitionTransactionError {
    CompetitionTransactionError::Failed("装备标记通知事实无效".to_owned())
}
