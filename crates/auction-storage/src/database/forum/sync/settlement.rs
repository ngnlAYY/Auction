//! 同一论坛快照内核对持久结算与最终 lot 事实；不计算曲线或净支付。

use auction_application::{ApplicationError, ForumLotSettlement};
use chrono::{DateTime, Utc};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

use super::storage_error;

pub(super) fn project(
    row: &PgRow,
    settled: bool,
) -> Result<Option<ForumLotSettlement>, ApplicationError> {
    if !settled {
        return Ok(None);
    }
    let status: String =
        row.try_get("status").map_err(|_| storage_error("读取结算 lot 状态失败"))?;
    let end_price: Option<i64> =
        row.try_get("settlement_end_price").map_err(|_| storage_error("读取持久落锤价失败"))?;
    let adjustment_ppm: Option<i64> =
        row.try_get("adjustment_ppm").map_err(|_| storage_error("读取持久结算调整失败"))?;
    let current_price: Option<i64> =
        row.try_get("current_price").map_err(|_| storage_error("读取最终领先金额失败"))?;
    let leader_id: Option<Uuid> =
        row.try_get("leader_user_id").map_err(|_| storage_error("读取最终赢家标识失败"))?;
    let leader_name: Option<String> =
        row.try_get("leader_name").map_err(|_| storage_error("读取最终赢家名称失败"))?;
    let mechanism: Option<String> =
        row.try_get("current_bid_mechanism").map_err(|_| storage_error("读取最终竞价来源失败"))?;
    let adjustment_ppm = adjustment_ppm
        .filter(|value| (-1_000_000..=1_000_000).contains(value))
        .ok_or_else(|| storage_error("终态快照缺少有效结算调整"))?;
    if status == "unsold" && end_price == Some(0) && leader_id.is_none() {
        return Ok(None);
    }
    if status != "sold"
        || end_price.is_none_or(|value| value <= 0)
        || end_price != current_price
        || leader_id.is_none()
        || leader_name.is_none()
        || !matches!(mechanism.as_deref(), Some("ordinary" | "proxy"))
    {
        return Err(storage_error("终态落锤价、赢家或来源与持久结算不一致"));
    }
    let leader_since: Option<DateTime<Utc>> =
        row.try_get("leader_since").map_err(|_| storage_error("读取最终连续领先时间失败"))?;
    let effective_end_at: Option<DateTime<Utc>> =
        row.try_get("effective_end_at").map_err(|_| storage_error("读取最终有效截止失败"))?;
    let (Some(leader_since), Some(effective_end_at)) = (leader_since, effective_end_at) else {
        return Err(storage_error("终态快照缺少权威连续领先时间"));
    };
    if leader_since > effective_end_at {
        return Err(storage_error("终态连续领先时间无效"));
    }
    Ok(Some(ForumLotSettlement { leader_since, adjustment_ppm }))
}
