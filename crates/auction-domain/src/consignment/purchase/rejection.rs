//! 购买的确定业务拒绝闭集及其稳定码与状态码。

/// 购买被确定拒绝的原因；码与状态随账本回执保存，不得改字节。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurchaseRejection {
    ListingNotActive,
    OperationsHold,
    StaleVersion,
    PriceChanged,
    EquipmentCommitted,
    CustodyUnproven,
    SeasonClosed,
    BotParty,
    SelfPurchase,
    ParticipantIneligible,
    FundingUnavailable,
}

impl PurchaseRejection {
    /// 全部拒绝，按判定优先序排列；码表与向量逐项对应。
    pub const ALL: [Self; 11] = [
        Self::ListingNotActive,
        Self::OperationsHold,
        Self::StaleVersion,
        Self::PriceChanged,
        Self::EquipmentCommitted,
        Self::CustodyUnproven,
        Self::SeasonClosed,
        Self::BotParty,
        Self::SelfPurchase,
        Self::ParticipantIneligible,
        Self::FundingUnavailable,
    ];

    /// 账本与 API 使用的稳定业务码。
    pub const fn code(self) -> &'static str {
        match self {
            Self::ListingNotActive => "listing_not_active",
            Self::OperationsHold => "operations_hold",
            Self::StaleVersion => "stale_version",
            Self::PriceChanged => "price_changed",
            Self::EquipmentCommitted => "equipment_committed",
            Self::CustodyUnproven => "custody_unproven",
            Self::SeasonClosed => "season_closed",
            Self::BotParty => "bot_party",
            Self::SelfPurchase => "self_purchase",
            Self::ParticipantIneligible => "participant_ineligible",
            Self::FundingUnavailable => "funding_unavailable",
        }
    }

    /// 首次写入决定的 HTTP 状态；`funding_unavailable` 只用于确定的写前拒绝。
    pub const fn http_status(self) -> u16 {
        match self {
            Self::ParticipantIneligible | Self::SelfPurchase | Self::BotParty => 403,
            Self::FundingUnavailable => 503,
            Self::ListingNotActive
            | Self::OperationsHold
            | Self::StaleVersion
            | Self::PriceChanged
            | Self::EquipmentCommitted
            | Self::CustodyUnproven
            | Self::SeasonClosed => 409,
        }
    }
}
