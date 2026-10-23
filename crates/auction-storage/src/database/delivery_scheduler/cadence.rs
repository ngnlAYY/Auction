//! 七类 Send 的持久起点；仅在 sending fence 同事务消费间隔，准备失败不占用节奏。

use auction_application::{DeliverySendIntervals, DeliveryWorkClaim};
use sqlx::{Postgres, Transaction};

use super::cash::FenceError;

/// 分组锁先于资金 scope/world/lot/delivery，所有发送路径使用同一顺序。
/// 领取时的时间过滤仅减轻无效准备；这里持锁复核才是并发与重启边界。
pub(super) async fn lock_due(
    tx: &mut Transaction<'_, Postgres>,
    claim: &DeliveryWorkClaim,
    intervals: &DeliverySendIntervals,
) -> Result<Option<String>, FenceError> {
    if claim.payload.get("operation").and_then(serde_json::Value::as_str) != Some("send") {
        return Ok(None);
    }
    let category = sqlx::query_scalar::<_, String>(
        "SELECT category FROM public.delivery_send_cadence
         WHERE category = public.delivery_send_category($1, $2) FOR UPDATE",
    )
    .bind(&claim.origin_kind)
    .bind(&claim.payload)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FenceError::Rejected)?;
    let seconds = intervals
        .values()
        .into_iter()
        .find_map(|(key, seconds)| (key == category).then_some(seconds))
        .filter(|seconds| *seconds <= 86_400)
        .and_then(|seconds| i64::try_from(seconds).ok())
        .ok_or(FenceError::Rejected)?;
    let due: bool = sqlx::query_scalar(
        "SELECT last_started_at IS NULL
             OR last_started_at + $2::bigint * interval '1 second' <= clock_timestamp()
         FROM public.delivery_send_cadence WHERE category = $1",
    )
    .bind(&category)
    .bind(seconds)
    .fetch_one(&mut **tx)
    .await?;
    if !due {
        return Err(FenceError::Rejected);
    }
    Ok(Some(category))
}

/// 只有已确定 Sending 的事务推进高水位；冻结、拒绝、回滚不消耗间隔。
pub(super) async fn record_started(
    tx: &mut Transaction<'_, Postgres>,
    category: &str,
) -> Result<(), FenceError> {
    let changed = sqlx::query(
        "UPDATE public.delivery_send_cadence
         SET last_started_at = GREATEST(last_started_at, clock_timestamp())
         WHERE category = $1",
    )
    .bind(category)
    .execute(&mut **tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(FenceError::Rejected);
    }
    Ok(())
}
