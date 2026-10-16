//! 论坛图片使用的公开时间快照；不携带装备、凭据或可变命令能力。

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// 与公开拍卖可见性一致的倒计时事实；适配器须在同一次读取中取得数据库时钟。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAuctionCountdown {
    pub auction_id: Uuid,
    /// 已实际开拍时取 opened_at，否则取 scheduled_open_at。
    pub intake_closes_at: DateTime<Utc>,
    /// 只表示本场计划截止，不合并单件 soft-close；预览态不提前公开截止。
    pub scheduled_end_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
}

impl PublicAuctionCountdown {
    /// 时间必须可精确表示，异常快照不得渲染成看似有效的计时器。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.auction_id.is_nil()
            || self.intake_closes_at.timestamp_nanos_opt().is_none()
            || self.observed_at.timestamp_nanos_opt().is_none()
            || self.scheduled_end_at.is_some_and(|time| time.timestamp_nanos_opt().is_none())
        {
            return Err("公开倒计时快照无效");
        }
        Ok(())
    }
}
