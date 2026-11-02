//! 公开倒计时的 PostgreSQL 读取；单条查询绑定时间事实、可见性与数据库时钟。

use auction_application::{ApplicationError, PublicAuctionCountdown};
use sqlx::Row;
use uuid::Uuid;

use super::{super::PostgresStore, mapping::storage_error};

pub(super) async fn read(
    store: &PostgresStore,
    auction_id: Uuid,
) -> Result<Option<PublicAuctionCountdown>, ApplicationError> {
    // 与公开目录一致：隐藏草稿/取消场次，禁用世界只留 settled 历史，不读 lot、正文或装备 key。
    // 后继预览不能借图片绕过当前活动场次的门禁。
    let row = sqlx::query(concat!(
        "SELECT a.id,
                COALESCE(a.opened_at, a.scheduled_open_at) AS intake_closes_at,
                CASE WHEN a.status = 'preview' THEN NULL ELSE a.scheduled_end_at END
                    AS scheduled_end_at,
                clock_timestamp() AS observed_at
         FROM public.auctions AS a
         JOIN public.worlds AS w ON w.id = a.world_id
         WHERE a.id = $1
           AND (w.enabled OR a.status = 'settled')
           AND a.status IN ('preview', 'live', 'final', 'settling', 'settled')
           AND ",
        super::preview_visibility_sql!()
    ))
    .bind(auction_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取公开倒计时失败"))?;
    let Some(row) = row else { return Ok(None) };
    let snapshot = PublicAuctionCountdown {
        auction_id: row.try_get("id").map_err(|_| storage_error("读取倒计时拍卖标识失败"))?,
        intake_closes_at: row
            .try_get("intake_closes_at")
            .map_err(|_| storage_error("读取倒计时收件截止失败"))?,
        scheduled_end_at: row
            .try_get("scheduled_end_at")
            .map_err(|_| storage_error("读取倒计时计划截止失败"))?,
        observed_at: row
            .try_get("observed_at")
            .map_err(|_| storage_error("读取倒计时观测时间失败"))?,
    };
    snapshot.validate().map_err(storage_error)?;
    Ok(Some(snapshot))
}
