//! 已接受的竞价事实与其产生机制。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{BidAmount, BidId, CommandId, DomainError, LotId, UserId};

/// 公开竞价事实的产生机制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BidMechanism {
    Ordinary,
    Proxy,
}

/// 已接受的不可变竞价事实，后续由应用层与审计、事件一起原子提交。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedBid {
    pub id: BidId,
    pub command_id: CommandId,
    pub lot_id: LotId,
    pub bidder_id: UserId,
    pub fact_index: u32,
    #[serde(deserialize_with = "crate::bidding::amount::deserialize_persisted_positive")]
    pub amount: BidAmount,
    pub effective_end_at: DateTime<Utc>,
    pub state_version: u64,
    pub mechanism: BidMechanism,
}

impl AcceptedBid {
    /// 校验竞价事实在写入审计和事件前具备完整身份与版本。
    pub fn validate(self) -> Result<(), DomainError> {
        if self.id.is_nil()
            || self.command_id.is_nil()
            || self.lot_id.is_nil()
            || self.bidder_id.is_nil()
        {
            return Err(DomainError::InvalidBidState("竞价事实缺少有效标识"));
        }
        if self.fact_index == 0 || self.state_version == 0 {
            return Err(DomainError::InvalidBidState("竞价事实版本必须为正数"));
        }
        if self.effective_end_at.timestamp_nanos_opt().is_none() {
            return Err(DomainError::InvalidBidState("竞价事实结束时间无效"));
        }

        Ok(())
    }
}
