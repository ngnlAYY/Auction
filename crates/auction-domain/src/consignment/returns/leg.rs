//! 退回腿：同一轮里可能被 RTS 退回的资产种类。

use super::errors::ReturnRuleError;
use crate::consignment::funding::BlockedLeg;

/// 退回腿；同一轮同一腿只接一件（`UNIQUE(合同, 义务, 原轮, 腿)`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReturnLeg {
    /// 原件装备。
    Original,
    /// 未付载体装备。
    Carrier,
    /// 现金退款 P。
    CashRefund,
    /// 现金卖款 S。
    CashPayout,
    /// 未付载体本金 1。
    CashPrincipal,
}

/// 消费一件退回时实际落到的资产效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumeEffect {
    /// 收回原件。
    RecoverOriginal,
    /// 收回载体。
    RecoverCarrier,
    /// 现金回补：同事务写资金回补并解除 UID 阻断。
    ReturnCash,
}

impl ReturnLeg {
    /// 全部腿；与持久闭集逐项对应。
    pub const ALL: [Self; 5] =
        [Self::Original, Self::Carrier, Self::CashRefund, Self::CashPayout, Self::CashPrincipal];

    /// 稳定持久码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Carrier => "carrier",
            Self::CashRefund => "cash_refund",
            Self::CashPayout => "cash_payout",
            Self::CashPrincipal => "cash_principal",
        }
    }

    /// 闭集解析，未知值失败关闭。
    pub fn parse(value: &str) -> Result<Self, ReturnRuleError> {
        Self::ALL.into_iter().find(|leg| leg.as_str() == value).ok_or(ReturnRuleError::UnknownLeg)
    }

    /// 是否现金腿。
    pub const fn is_cash(self) -> bool {
        self.blocked_leg().is_some()
    }

    /// 现金腿对应的 UID 阻断腿；实物腿没有。
    pub const fn blocked_leg(self) -> Option<BlockedLeg> {
        match self {
            Self::CashRefund => Some(BlockedLeg::Refund),
            Self::CashPayout => Some(BlockedLeg::Payout),
            Self::CashPrincipal => Some(BlockedLeg::CarrierPrincipal),
            Self::Original | Self::Carrier => None,
        }
    }

    /// 消费该腿的资产效果。
    pub const fn effect(self) -> ConsumeEffect {
        match self {
            Self::Original => ConsumeEffect::RecoverOriginal,
            Self::Carrier => ConsumeEffect::RecoverCarrier,
            Self::CashRefund | Self::CashPayout | Self::CashPrincipal => ConsumeEffect::ReturnCash,
        }
    }
}
