//! 定义一口价挂单、挂单模式、状态与版本。

use std::str::FromStr;

use auction_time::{Duration, Timestamp};

use crate::consignment::errors::{ListingError, ListingReason};

/// 合同第 2.1 和第 6 节规定的解析总期限，自登记起固定 24 小时。
pub const LOOKUP_TTL: Duration = Duration::hours(24);
/// 合同第 1 和第 6 节规定的非寄售默认展示天数，可由配置调整。
pub const DEFAULT_DISPLAY_TTL_DAYS: i64 = 30;
/// 合同第 1 和第 6 节规定的寄售默认待寄天数，可由配置调整。
pub const DEFAULT_DEPOSIT_TTL_DAYS: i64 = 7;

/// 登记时确定的挂单模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListingMode {
    /// 非寄售，由卖家持有装备并与买家自行交易。
    Direct,
    /// 寄售，前三期核验通过后进入待寄。
    Consigned,
}

impl ListingMode {
    /// 返回数据库和 DTO 共用的稳定模式码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Consigned => "consigned",
        }
    }
}

impl FromStr for ListingMode {
    type Err = ListingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "direct" => Ok(Self::Direct),
            "consigned" => Ok(Self::Consigned),
            // 不变量 6、7：现有错误闭集用固定动作码区分编码失败，不回显原始输入。
            _ => Err(ListingError::InvalidTransition { from: "unknown", action: "decode_mode" }),
        }
    }
}

/// 前三期的一口价挂单状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListingState {
    /// 登记已受理，等待装备核验。
    Resolving,
    /// 非寄售挂单已通过核验并展示。
    Displayed,
    /// 寄售挂单已通过核验，平台尚未代管装备。
    AwaitingDeposit,
    /// 整封接受已完成，平台承担取得或核对责任。
    DepositAccepted,
    /// 唯一可购买态；已取得实物并完成上架决策。
    Available,
    /// 已有成交合同，履行中。
    SalePending,
    /// 已取得实物但暂不上架，原因见 `HeldOffMarketReason`。
    HeldOffMarket,
    /// 卖家已登记撤回意愿，等待后期 Withdrawal 合同执行。
    WithdrawalPending,
    /// 核验或解析失败，永久终结。
    Rejected,
    /// 达到本态截止，永久终结。
    Expired,
    /// 主动或系统关闭，永久终结。
    Closed,
}

impl ListingState {
    /// 返回数据库和 DTO 共用的稳定状态码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resolving => "resolving",
            Self::Displayed => "displayed",
            Self::AwaitingDeposit => "awaiting_deposit",
            Self::DepositAccepted => "deposit_accepted",
            Self::Available => "available",
            Self::SalePending => "sale_pending",
            Self::HeldOffMarket => "held_off_market",
            Self::WithdrawalPending => "withdrawal_pending",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Closed => "closed",
        }
    }

    /// 判断持久记录是否处于活动态，有效状态还须结合强制终止事实。
    pub const fn is_active(self) -> bool {
        !matches!(self, Self::Rejected | Self::Expired | Self::Closed)
    }

    /// 是否属于第 4 期新增的代管状态；旧 v1 变更不对这些状态执行新语义。
    pub const fn is_custody_managed(self) -> bool {
        matches!(
            self,
            Self::DepositAccepted
                | Self::Available
                | Self::SalePending
                | Self::HeldOffMarket
                | Self::WithdrawalPending
        )
    }

    /// 是否占用活动额度；`available` 占一个名额，`held_off_market` 与其他代管态不占。
    pub const fn occupies_quota(self) -> bool {
        matches!(self, Self::Resolving | Self::Displayed | Self::AwaitingDeposit | Self::Available)
    }
}

impl FromStr for ListingState {
    type Err = ListingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "resolving" => Ok(Self::Resolving),
            "displayed" => Ok(Self::Displayed),
            "awaiting_deposit" => Ok(Self::AwaitingDeposit),
            "deposit_accepted" => Ok(Self::DepositAccepted),
            "available" => Ok(Self::Available),
            "sale_pending" => Ok(Self::SalePending),
            "held_off_market" => Ok(Self::HeldOffMarket),
            "withdrawal_pending" => Ok(Self::WithdrawalPending),
            "rejected" => Ok(Self::Rejected),
            "expired" => Ok(Self::Expired),
            "closed" => Ok(Self::Closed),
            _ => Err(ListingError::InvalidTransition { from: "unknown", action: "decode_state" }),
        }
    }
}

/// 以 main Credits 计且符合合同第 2.2 节范围的整数标价。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ListingPrice(i64);

impl ListingPrice {
    /// 最低可登记标价。
    pub const MIN: i64 = 1;
    /// 最高可登记标价。
    pub const MAX: i64 = 1_000_000_000;

    /// 校验并构造标价。
    pub fn new(credits: i64) -> Result<Self, ListingError> {
        // 不变量 6：奇数价格合法，越界金额直接拒绝。
        if !(Self::MIN..=Self::MAX).contains(&credits) {
            return Err(ListingError::InvalidPrice);
        }
        Ok(Self(credits))
    }

    /// 返回整数 Credits。
    pub const fn value(self) -> i64 {
        self.0
    }
}

/// 可由数据库及浏览器精确表示的挂单事实版本，范围见合同第 4.1 节。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ListingVersion(u64);

impl ListingVersion {
    /// 首次登记的版本。
    pub const FIRST: Self = Self(1);
    /// 数据库和 DTO 的共同安全上限。
    pub const MAX: u64 = 9_007_199_254_740_991;

    /// 校验并构造持久版本或命令所携的预期版本。
    pub fn new(state_version: u64) -> Result<Self, ListingError> {
        if !(Self::FIRST.0..=Self::MAX).contains(&state_version) {
            return Err(ListingError::InvalidTransition {
                from: "unknown",
                action: "decode_version",
            });
        }
        Ok(Self(state_version))
    }

    /// 返回数据库或 DTO 中的整数版本。
    pub const fn value(self) -> u64 {
        self.0
    }

    /// 不变量 3、6：版本达到共同上限时拒绝产生新的事实版本。
    pub(super) fn next(self) -> Option<Self> {
        (self.0 < Self::MAX).then_some(Self(self.0 + 1))
    }
}

/// 迁移所需的最小持久快照，时刻由调用方从数据库事实中取得。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingSnapshot {
    /// 登记时确定的模式。
    pub mode: ListingMode,
    /// 已提交的状态。
    pub state: ListingState,
    /// 已提交的事实版本。
    pub state_version: ListingVersion,
    /// 当前标价。
    pub price: ListingPrice,
    /// 当前状态的冻结截止，终结后保留进入终态前的值。
    pub expires_at: Timestamp,
    /// 首次提交的终态原因，活动态为空。
    pub reason: Option<ListingReason>,
}

#[cfg(test)]
mod tests {
    use crate::consignment::errors::ListingError;
    use crate::consignment::listing::{ListingMode, ListingPrice, ListingState, ListingVersion};

    #[test]
    fn listing_mode_codes_round_trip_and_reject_unknown() {
        for (mode, code) in [(ListingMode::Direct, "direct"), (ListingMode::Consigned, "consigned")]
        {
            assert_eq!(mode.as_str(), code);
            assert_eq!(code.parse(), Ok(mode));
        }
        for code in ["", "Direct", "direct ", "display_only"] {
            assert_eq!(
                code.parse::<ListingMode>(),
                Err(ListingError::InvalidTransition { from: "unknown", action: "decode_mode" })
            );
        }
    }

    #[test]
    fn listing_state_codes_round_trip_and_reject_unknown() {
        for (state, code, active) in [
            (ListingState::Resolving, "resolving", true),
            (ListingState::Displayed, "displayed", true),
            (ListingState::AwaitingDeposit, "awaiting_deposit", true),
            (ListingState::DepositAccepted, "deposit_accepted", true),
            (ListingState::Available, "available", true),
            (ListingState::SalePending, "sale_pending", true),
            (ListingState::HeldOffMarket, "held_off_market", true),
            (ListingState::WithdrawalPending, "withdrawal_pending", true),
            (ListingState::Rejected, "rejected", false),
            (ListingState::Expired, "expired", false),
            (ListingState::Closed, "closed", false),
        ] {
            assert_eq!(state.as_str(), code);
            assert_eq!(code.parse(), Ok(state));
            assert_eq!(state.is_active(), active);
            assert_eq!(
                state.is_custody_managed(),
                matches!(
                    state,
                    ListingState::DepositAccepted
                        | ListingState::Available
                        | ListingState::SalePending
                        | ListingState::HeldOffMarket
                        | ListingState::WithdrawalPending
                )
            );
            assert_eq!(
                state.occupies_quota(),
                matches!(
                    state,
                    ListingState::Resolving
                        | ListingState::Displayed
                        | ListingState::AwaitingDeposit
                        | ListingState::Available
                )
            );
        }
        for code in ["", "Closed", "closed ", "registered_only", "awaiting-deposit"] {
            assert_eq!(
                code.parse::<ListingState>(),
                Err(ListingError::InvalidTransition { from: "unknown", action: "decode_state" })
            );
        }
    }

    #[test]
    fn listing_price_validates_integer_bounds() {
        for credits in [1, 101, 1_000_000_000] {
            assert_eq!(ListingPrice::new(credits).map(ListingPrice::value), Ok(credits));
        }
        for credits in [-1, 0, 1_000_000_001, i64::MAX] {
            assert_eq!(ListingPrice::new(credits), Err(ListingError::InvalidPrice));
        }
    }

    #[test]
    fn listing_version_validates_safe_integer_bounds() {
        assert_eq!(ListingVersion::FIRST.value(), 1);
        for version in [1, 9_007_199_254_740_991] {
            assert_eq!(ListingVersion::new(version).map(ListingVersion::value), Ok(version));
        }
        for version in [0, 9_007_199_254_740_992, u64::MAX] {
            assert_eq!(
                ListingVersion::new(version),
                Err(ListingError::InvalidTransition { from: "unknown", action: "decode_version" })
            );
        }
    }
}
