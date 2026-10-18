//! Bot 内部履约只选择原义务的执行方式，不重算冻结金额或创建新的责任。

use crate::{SettlementObligationKind, SettlementWorld};

/// 同账号收件义务允许的内部动作；资金授权和当前持有证明由应用/存储边界提供。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalSettlementAction {
    KeepUnsold,
    Purchase,
    Handoff,
    RetainSeller,
    Refund,
}

impl InternalSettlementAction {
    /// 撤拍返还与流拍返还在收件人即本 bot 时都直接留存；外部收件人仍走寄回投递。
    pub const fn for_kind(kind: SettlementObligationKind) -> Option<Self> {
        match kind {
            SettlementObligationKind::UnsoldReturn => Some(Self::KeepUnsold),
            SettlementObligationKind::WinnerEquipmentCod
            | SettlementObligationKind::WinnerPaymentCarrierCod => Some(Self::Purchase),
            SettlementObligationKind::WinnerEquipmentHandoff => Some(Self::Handoff),
            SettlementObligationKind::SellerPayout => Some(Self::RetainSeller),
            SettlementObligationKind::BuyerRefund => Some(Self::Refund),
            SettlementObligationKind::WithdrawnReturn => Some(Self::KeepUnsold),
        }
    }

    /// 核验原义务的派发/装备世界与冻结金额形状，不计算价格、税或补偿。
    pub const fn matches_frozen_shape(
        kind: SettlementObligationKind,
        dispatch: SettlementWorld,
        equipment: SettlementWorld,
        amount: i64,
        carrier: u32,
    ) -> bool {
        match kind {
            SettlementObligationKind::WinnerEquipmentCod => {
                matches!(dispatch, SettlementWorld::Main)
                    && matches!(equipment, SettlementWorld::Main)
                    && amount > 0
                    && carrier == 0
            }
            SettlementObligationKind::WinnerPaymentCarrierCod => {
                matches!(dispatch, SettlementWorld::Main)
                    && matches!(equipment, SettlementWorld::Isekai)
                    && amount > 0
                    && carrier == 1
            }
            SettlementObligationKind::WinnerEquipmentHandoff => {
                matches!(dispatch, SettlementWorld::Isekai)
                    && matches!(equipment, SettlementWorld::Isekai)
                    && amount == 0
                    && carrier == 0
            }
            SettlementObligationKind::SellerPayout | SettlementObligationKind::BuyerRefund => {
                matches!(dispatch, SettlementWorld::Main) && amount > 0 && carrier == 0
            }
            SettlementObligationKind::UnsoldReturn => {
                matches!(
                    (dispatch, equipment),
                    (SettlementWorld::Main, SettlementWorld::Main)
                        | (SettlementWorld::Isekai, SettlementWorld::Isekai)
                ) && amount == 0
                    && carrier == 0
            }
            SettlementObligationKind::WithdrawnReturn => {
                matches!(
                    (dispatch, equipment),
                    (SettlementWorld::Main, SettlementWorld::Main)
                        | (SettlementWorld::Isekai, SettlementWorld::Isekai)
                ) && amount == 0
                    && carrier == 0
            }
        }
    }

    /// 付款前和 Isekai 交接时各自重新证明当前持有；卖方现金留存不要求装备仍在库。
    pub const fn requires_equipment(self) -> bool {
        !matches!(self, Self::RetainSeller)
    }

    /// 内部 carrier 不虚构 1C 收支；现金变化完全委托冻结 B/C/S/T 的资金 owner。
    pub const fn requires_funding(self) -> bool {
        !matches!(self, Self::KeepUnsold | Self::Handoff)
    }
}
