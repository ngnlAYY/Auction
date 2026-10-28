//! 领域闭集枚举的稳定名称表。
//!
//! 稳定名称同时用于持久化列、确定性标识派生、审计元数据与配置，已发布值不可改名；
//! 每个枚举只在此声明一份名称表，正反两个方向均由同一表生成，避免各层手写映射漂移。

/// 为闭集枚举生成 `as_str`（稳定名称）与 `from_name`（未知值返回 `None`，由调用方失败关闭）。
macro_rules! stable_names {
    ($($ty:ty { $($variant:ident => $name:literal),+ $(,)? })+) => {$(
        impl $ty {
            /// 持久化、审计与配置共用的稳定名称；已发布值不可改名。
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }

            /// 解析稳定名称；未知值返回 `None`，由调用方失败关闭。
            pub fn from_name(value: &str) -> Option<Self> {
                match value {
                    $($name => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    )+};
}

stable_names! {
    crate::AuctionStatus {
        Draft => "draft",
        Preview => "preview",
        Live => "live",
        Final => "final",
        Settling => "settling",
        Settled => "settled",
        Cancelled => "cancelled",
    }
    crate::BidMechanism {
        Ordinary => "ordinary",
        Proxy => "proxy",
    }
    crate::ProxyMode {
        Fast => "fast",
        Slow => "slow",
    }
    crate::ProxyStatus {
        Active => "active",
        FinalFast => "final_fast",
        Exhausted => "exhausted",
        Frozen => "frozen",
        Closed => "closed",
    }
    crate::ProxyTerminalReason {
        StrictlyDefeated => "strictly_defeated",
        RankedBelow => "ranked_below",
        EligibilityLost => "eligibility_lost",
        CompetitionClosed => "competition_closed",
    }
    crate::ProxyTransitionKind {
        Created => "created",
        Lowered => "lowered",
        FinalFast => "final_fast",
        Exhausted => "exhausted",
        Frozen => "frozen",
        Closed => "closed",
    }
    crate::RecoveryMode {
        RemindV1 => "remind_v1",
    }
    crate::SettlementMode {
        LegacyZeroV0 => "legacy_zero_v0",
        HoldCurveV1 => "hold_curve_v1",
        HoldCurveV2 => "hold_curve_v2",
    }
    crate::SettlementObligationKind {
        WinnerEquipmentCod => "winner_equipment_cod",
        WinnerPaymentCarrierCod => "winner_payment_carrier_cod",
        WinnerEquipmentHandoff => "winner_equipment_handoff",
        SellerPayout => "seller_payout",
        BuyerRefund => "buyer_refund",
        UnsoldReturn => "unsold_return",
        WithdrawnReturn => "withdrawn_return",
    }
    crate::SettlementObligationState {
        Ready => "ready",
        Blocked => "blocked",
        Contingent => "contingent",
    }
    crate::SettlementWorld {
        Main => "main",
        Isekai => "isekai",
    }
}
