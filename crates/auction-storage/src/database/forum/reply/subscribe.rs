//! 论坛 `notify` 改为订阅该 lot 的被超提醒，不再寄导入回执。
//!
//! 订阅按用户 × lot 唯一，天然去重；每帖与每账号每场的订阅数量在锁内计上限，
//! 超额只返回拒绝结果，由系统日志楼展示，不写入邮件、不改变竞价事实。

#[cfg(feature = "postgres")]
use auction_application::core::ApplicationError;
#[cfg(feature = "postgres")]
use auction_application::forum::NotifySubscription;
#[cfg(feature = "postgres")]
use auction_domain::{AuctionId, LotId, UserId};
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

#[cfg(feature = "postgres")]
use super::super::super::DatabaseStore;
#[cfg(feature = "postgres")]
use super::validation::storage_error;

/// 单帖最多建立这么多条订阅；超出只记系统日志楼。
#[cfg(feature = "postgres")]
pub(super) const MAX_SUBSCRIPTIONS_PER_POST: i64 = 8;
/// 每账号每场最多关注这么多 lot；超出只记系统日志楼。
#[cfg(feature = "postgres")]
pub(super) const MAX_SUBSCRIPTIONS_PER_ACCOUNT_AUCTION: i64 = 64;

#[cfg(feature = "postgres")]
pub(super) async fn subscribe_notify_postgres(
    store: &DatabaseStore,
    command_id: Uuid,
    auction_id: AuctionId,
    lot_id: LotId,
    user_id: UserId,
    post_id: &str,
    lot_code: &str,
) -> Result<NotifySubscription, ApplicationError> {
    use sqlx::Row;

    if command_id.is_nil()
        || auction_id.as_uuid().is_nil()
        || lot_id.as_uuid().is_nil()
        || user_id.as_uuid().is_nil()
        || post_id.is_empty()
        || lot_code.is_empty()
    {
        return Err(storage_error("论坛订阅参数无效"));
    }
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启论坛订阅事务失败"))?;
    // 本帖其它 notify 命令（pending 重试不计入）已占用的订阅名额。
    let in_post: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.auction_forum_bid_commands
         WHERE post_id = $1 AND command_kind = 'notify' AND command_id <> $2",
    )
    .bind(post_id)
    .bind(command_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取论坛帖订阅数量失败"))?;
    if in_post >= MAX_SUBSCRIPTIONS_PER_POST {
        transaction
            .rollback()
            .await
            .map_err(|error| storage_error_from(error, "回滚论坛订阅事务失败"))?;
        return Ok(NotifySubscription::Capped("post_limit"));
    }
    let existing = sqlx::query(
        "SELECT watched FROM public.lot_watches WHERE user_id = $1 AND lot_id = $2 FOR UPDATE",
    )
    .bind(user_id.as_uuid())
    .bind(lot_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取论坛订阅现状失败"))?;
    let already = existing
        .as_ref()
        .map(|row| row.try_get::<bool, _>("watched").unwrap_or(false))
        .unwrap_or(false);
    if already {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交论坛订阅事务失败"))?;
        return Ok(NotifySubscription::AlreadySubscribed);
    }
    let watched: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.lot_watches watch
         JOIN public.lots lot ON lot.id = watch.lot_id
         WHERE watch.user_id = $1 AND watch.watched AND lot.auction_id = $2",
    )
    .bind(user_id.as_uuid())
    .bind(auction_id.as_uuid())
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取账号订阅数量失败"))?;
    if watched >= MAX_SUBSCRIPTIONS_PER_ACCOUNT_AUCTION {
        transaction
            .rollback()
            .await
            .map_err(|error| storage_error_from(error, "回滚论坛订阅事务失败"))?;
        return Ok(NotifySubscription::Capped("account_limit"));
    }
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO public.lot_watches (user_id, lot_id, watched, channel, version)
         VALUES ($1, $2, true, 'mm', 1)
         ON CONFLICT (user_id, lot_id) DO UPDATE SET watched = true,
             version = lot_watches.version + 1, updated_at = clock_timestamp()
         RETURNING version",
    )
    .bind(user_id.as_uuid())
    .bind(lot_id.as_uuid())
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入论坛订阅失败"))?;
    sqlx::query(
        "INSERT INTO public.audit_records
         (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
         VALUES ($1, 'system', $2, 'lot_watch.forum_notify', 'lot', $3, 'success', 'worker', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(user_id.as_uuid().to_string())
    .bind(lot_id.as_uuid().to_string())
    .bind(serde_json::json!({
        "command_id": command_id,
        "post_id": post_id,
        "lot_code": lot_code,
        "version": version,
    }))
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入论坛订阅审计失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交论坛订阅事务失败"))?;
    Ok(NotifySubscription::Subscribed)
}
