//! 竞价事务内的 PM 来源及队列；缺收件人明确 suppressed，不吞 SQL 错误。

use super::*;
use auction_application::{LotWatchPmNotification, lot_watch_pm_notification};
use auction_domain::AcceptedBid;
use chrono::{DateTime, Utc};

/// 竞价快照，不从当前用户名推导论坛收件人。
pub(super) struct PmFact {
    pub user_id: Uuid,
    pub cycle: u64,
    pub watch_version: i64,
    pub preference_version: i64,
    pub recipient: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

pub(super) async fn notify_pm(
    transaction: &mut Transaction<'_, Postgres>,
    bid: &AcceptedBid,
    fact: PmFact,
) -> Result<(), CompetitionTransactionError> {
    let lot_id = bid.lot_id.as_uuid();
    let LotWatchPmNotification { notification_id, recipient, payload, digest } =
        lot_watch_pm_notification(
            lot_id,
            fact.user_id,
            fact.cycle,
            bid.amount.value(),
            fact.recipient,
        )
        .map_err(|_| invalid())?;
    sqlx::query("INSERT INTO public.lot_watch_notifications (id,user_id,lot_id,leadership_cycle,displaced_bid_id,watch_version,command_id,request_digest,recipient,payload,created_at,channel,preference_version)
        VALUES ($1,$2,$3,$4,$5,$6,$1,$7,$8,$9,$10,'pm',$11)")
        .bind(notification_id).bind(fact.user_id).bind(lot_id).bind(i64::try_from(fact.cycle).map_err(|_| invalid())?)
        .bind(bid.id.as_uuid()).bind(fact.watch_version).bind(digest.as_bytes().to_vec()).bind(&recipient).bind(payload).bind(fact.occurred_at).bind(fact.preference_version)
        .execute(&mut **transaction).await.map_err(sql_error)?;
    sqlx::query("INSERT INTO public.lot_watch_pm_deliveries (notification_id,status,failure_code,completed_at)
        VALUES ($1,$2,$3,CASE WHEN $2 = 'suppressed' THEN clock_timestamp() ELSE NULL END)")
        .bind(notification_id).bind(if recipient.is_some() {"pending"} else {"suppressed"})
        .bind(if recipient.is_some() {None} else {Some("recipient_missing")})
        .execute(&mut **transaction).await.map_err(sql_error)?;
    audit(transaction, fact.user_id,lot_id,"lot_watch.outbid",serde_json::json!({"notification_id":notification_id,"cycle":fact.cycle,"channel":"pm","suppressed":recipient.is_none()})).await
}
fn invalid() -> CompetitionTransactionError {
    CompetitionTransactionError::Failed("PM 通知事实无效".to_owned())
}
