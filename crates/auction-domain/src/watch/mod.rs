//! 装备标记的渠道与连续领先周期；不依赖外部投递或持久化。

use crate::UserId;

/// 账号提醒渠道闭集；旧单件标记接口仍只接受 MM 兼容占位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LotWatchChannel {
    Mm,
    Pm,
}

impl LotWatchChannel {
    /// 账号偏好允许 MM 或 PM，不允许同一事件双发。
    pub fn preference(value: &str) -> Option<Self> {
        match value {
            "mm" => Some(Self::Mm),
            "pm" => Some(Self::Pm),
            _ => None,
        }
    }

    /// 旧单件标记兼容边界；不能据此解释账号实际通知方式。
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
