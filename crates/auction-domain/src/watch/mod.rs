//! 装备标记的渠道与连续领先周期；不依赖外部投递或持久化。

use crate::UserId;

/// 单条提醒的投递渠道闭集；旧单件标记接口仍只接受 MM 兼容占位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LotWatchChannel {
    Mm,
    Pm,
}

impl LotWatchChannel {
    /// 提醒事实与待定事实里的稳定渠道名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::Pm => "pm",
        }
    }

    /// 从提醒事实里的渠道名还原单个渠道；账号偏好用 [`LotWatchChannels::preference`]。
    pub fn parse(value: &str) -> Option<Self> {
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

/// 账号提醒渠道偏好：MM 与 PM 各自独立开关，可以同时启用，也可以都关闭。
///
/// 启用的每个渠道在同一领先周期各自最多一条提醒；都关闭时标记只作收藏，不产生提醒。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LotWatchChannels {
    pub mm: bool,
    pub pm: bool,
}

impl LotWatchChannels {
    /// 偏好的稳定存储与 wire 表示闭集：`none`、`mm`、`pm`、`both`；其余取值失败关闭。
    pub fn preference(value: &str) -> Option<Self> {
        let (mm, pm) = match value {
            "none" => (false, false),
            "mm" => (true, false),
            "pm" => (false, true),
            "both" => (true, true),
            _ => return None,
        };
        Some(Self { mm, pm })
    }

    pub const fn as_str(self) -> &'static str {
        match (self.mm, self.pm) {
            (false, false) => "none",
            (true, false) => "mm",
            (false, true) => "pm",
            (true, true) => "both",
        }
    }

    /// 按固定顺序（MM、PM）列出启用的渠道。
    pub fn enabled(self) -> impl Iterator<Item = LotWatchChannel> {
        [(self.mm, LotWatchChannel::Mm), (self.pm, LotWatchChannel::Pm)]
            .into_iter()
            .filter_map(|(enabled, channel)| enabled.then_some(channel))
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

#[cfg(test)]
mod tests {
    use super::{LotWatchChannel, LotWatchChannels};

    #[test]
    fn preference_round_trips_the_closed_set() {
        for value in ["none", "mm", "pm", "both"] {
            let channels = LotWatchChannels::preference(value).expect("closed set value");
            assert_eq!(channels.as_str(), value);
        }
        for value in ["", "MM", "mm,pm", "all", "sms"] {
            assert_eq!(LotWatchChannels::preference(value), None);
        }
    }

    #[test]
    fn enabled_channels_keep_a_fixed_order() {
        let enabled = |value: &str| {
            LotWatchChannels::preference(value)
                .expect("closed set value")
                .enabled()
                .collect::<Vec<_>>()
        };
        assert_eq!(enabled("none"), vec![]);
        assert_eq!(enabled("mm"), vec![LotWatchChannel::Mm]);
        assert_eq!(enabled("pm"), vec![LotWatchChannel::Pm]);
        assert_eq!(enabled("both"), vec![LotWatchChannel::Mm, LotWatchChannel::Pm]);
    }

    #[test]
    fn single_channel_names_do_not_accept_preference_values() {
        assert_eq!(LotWatchChannel::parse("mm"), Some(LotWatchChannel::Mm));
        assert_eq!(LotWatchChannel::parse("pm"), Some(LotWatchChannel::Pm));
        assert_eq!(LotWatchChannel::parse("both"), None);
        assert_eq!(LotWatchChannel::parse("none"), None);
        // 单件标记的兼容占位只认 MM。
        assert_eq!(LotWatchChannel::available("pm"), None);
    }
}
