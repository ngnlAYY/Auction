//! 装备标记的渠道与连续领先周期；不依赖外部投递或持久化。

use crate::UserId;

/// 提醒渠道闭集；PM 仅预留，不代表已装配传输。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LotWatchChannel {
    Mm,
    Pm,
}

impl LotWatchChannel {
    /// 当前唯一可用渠道；未知值与未装配 PM 均失败关闭。
    pub fn available(value: &str) -> Option<Self> {
        match value {
            "mm" => Some(Self::Mm),
            _ => None,
        }
    }
}

/// 连续领先周期由首次取得领先的竞价状态版本标识，同人加价不重开周期。
#[derive(Debug, Clone, Copy)]
pub struct LotLeadership {
    pub leader: Option<UserId>,
    pub cycle: u64,
}

impl LotLeadership {
    /// 按已接受事实顺序推进；返回被超越者及其旧周期，不能只比较事务最终快照。
    pub fn accept(&mut self, bidder: UserId, state_version: u64) -> Option<(UserId, u64)> {
        if self.leader == Some(bidder) {
            return None;
        }
        let displaced = self.leader.map(|leader| (leader, self.cycle));
        self.leader = Some(bidder);
        self.cycle = state_version;
        displaced
    }
}
