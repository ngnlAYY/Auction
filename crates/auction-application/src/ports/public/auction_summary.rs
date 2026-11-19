//! Public 按 ID 读取已结算场次的应用投影与窄端口。

use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::core::ApplicationFuture;

/// 已结算场次的公开摘要。有效结束时刻沿用目录的公开 lot 聚合口径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettledAuctionSummary {
    pub id: Uuid,
    pub world_id: String,
    pub sequence: i64,
    pub effective_end_at: Option<Timestamp>,
}

/// 读取已结算场次摘要的存储端口。
pub trait PublicAuctionSummaryReader: Send + Sync {
    /// 仅投影公开且已结算的场次。停用世界的已结算场次仍可读取。
    fn read_settled_auction_summary(
        &self,
        auction_id: Uuid,
    ) -> ApplicationFuture<'_, Option<SettledAuctionSummary>>;
}
