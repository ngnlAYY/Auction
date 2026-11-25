//! 派发证据类别闭集。

/// 只追加证据的类别；`UNIQUE(attempt, category, remote_observation_id)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvidenceCategory {
    /// Sent 箱绑定（唯一 Sent MID）。
    SentBinding,
    /// 买家确领原件。
    OriginalClaim,
    /// 载体付款。
    CarrierPayment,
    /// 现金确领。
    CashClaim,
    /// 系统退件。
    Rts,
}

impl EvidenceCategory {
    /// 全部类别；与持久闭集逐项对应。
    pub const ALL: [Self; 5] =
        [Self::SentBinding, Self::OriginalClaim, Self::CarrierPayment, Self::CashClaim, Self::Rts];

    /// 稳定持久码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SentBinding => "sent_binding",
            Self::OriginalClaim => "original_claim",
            Self::CarrierPayment => "carrier_payment",
            Self::CashClaim => "cash_claim",
            Self::Rts => "rts",
        }
    }

    /// 闭集解析，未知值失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|category| category.as_str() == value)
    }
}
