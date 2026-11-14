//! 顺序消费已提交的标记事件：自动标记与领先周期只由提交后的 outbox 处理应用。
//! 慢速代理仍可反击时只记待定被超提醒，能行动时才入队。
//! 账号的提醒渠道偏好是集合（MM、PM 各自独立）：每条来源与待定事实只属于一个渠道，
//! 启用的每个渠道在同一领先周期各自最多一条。

use auction_application::admin::AdminRuntimeMailTemplatesConfig;
use auction_application::core::CompetitionTransactionError;
use auction_domain::{AcceptedBid, LotLeadership, UserId};
use chrono::{DateTime, Utc};
use sqlx::Postgres;
use sqlx::Row;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::lot_watch::persistence::{audit, invalidate, sql_error};

mod outbid;
mod pending;

use outbid::notify;
pub(crate) use pending::resolve_pending;

/// 自动标记一条关注；由提交后的 outbox 在独立事务调用，绝不进入出价 Serializable 事务。
/// 事件发生之后用户已经显式修改过标记时保留用户意图，不推进版本也不覆盖取消。
pub(crate) async fn auto_watch(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    lot_id: Uuid,
    action: &str,
    occurred_at: DateTime<Utc>,
) -> Result<bool, CompetitionTransactionError> {
    let version: Option<i64> = sqlx::query_scalar(
        "INSERT INTO public.lot_watches
        (user_id, lot_id, watched, channel, version) VALUES ($1, $2, true, 'mm', 1)
        ON CONFLICT (user_id, lot_id) DO UPDATE SET watched = true,
            version = lot_watches.version + 1, updated_at = clock_timestamp()
        WHERE lot_watches.updated_at <= $3
        RETURNING version",
    )
    .bind(user_id)
    .bind(lot_id)
    .bind(occurred_at)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(sql_error)?;
    let Some(version) = version else {
        return Ok(false);
    };
    audit(transaction, user_id, lot_id, action, serde_json::json!({"version": version})).await?;
    Ok(true)
}

/// 应用一条已提交的竞价事实。返回本轮是否入队了至少一条新的 `lot_watch` 投递；
/// 调用方在事务提交后据此发送唤醒，通知不在事务内。
pub(crate) async fn apply_bid(
    transaction: &mut Transaction<'_, Postgres>,
    bid: &AcceptedBid,
    occurred_at: DateTime<Utc>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
    public_base_url: Option<&str>,
    mail_templates: &AdminRuntimeMailTemplatesConfig,
) -> Result<bool, CompetitionTransactionError> {
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
        cycle: u64::try_from(cycle).map_err(corrupt_value)?,
    };
    if leadership.leader == Some(bid.bidder_id) {
        return Ok(false);
    }
    let displaced = leadership.accept(bid.bidder_id, bid.state_version);
    invalidate(transaction, bid.bidder_id.as_uuid(), lot_id, "leading_again").await?;
    sqlx::query(
        "UPDATE public.lot_watch_leadership SET leader_user_id = $2, cycle = $3 WHERE lot_id = $1",
    )
    .bind(lot_id)
    .bind(bid.bidder_id.as_uuid())
    .bind(i64::try_from(leadership.cycle).map_err(corrupt_value)?)
    .execute(&mut **transaction)
    .await
    .map_err(sql_error)?;
    let mut wakeup = false;
    if let Some((user_id, cycle)) = displaced {
        wakeup |= notify(
            transaction,
            bid,
            user_id.as_uuid(),
            cycle,
            occurred_at,
            notifications,
            public_base_url,
            mail_templates,
        )
        .await?;
    }
    // 代理状态在本事务内已更新：慢速代理耗尽或被更高普通出价击败后，
    // 待定提醒在此刻入队；反超成功则退役待定事实。
    wakeup |= resolve_pending(
        transaction,
        lot_id,
        occurred_at,
        notifications,
        public_base_url,
        mail_templates,
    )
    .await?;
    Ok(wakeup)
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

fn corrupt() -> CompetitionTransactionError {
    CompetitionTransactionError::Failed("装备标记通知事实无效".to_owned())
}

/// 数值范围与模板渲染失败都归为同一类事实损坏；原错误不含可用于处置的信息。
fn corrupt_value<E>(_: E) -> CompetitionTransactionError {
    corrupt()
}
