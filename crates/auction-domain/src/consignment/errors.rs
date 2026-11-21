//! 一口价挂单的领域错误与持久化终态原因。

use thiserror::Error;

/// 挂单领域拒绝执行的原因，诊断上下文限于安全的领域值。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ListingError {
    /// 世界或装备编号不符合挂单身份规则。
    #[error("装备身份无效")]
    InvalidIdentity,
    /// 标价不在允许范围内。
    #[error("挂单标价无效")]
    InvalidPrice,
    /// 费率不符合挂单规则。
    #[error("挂单费率无效")]
    InvalidFee,
    /// 当前状态不允许执行指定动作。
    #[error("挂单处于 {from} 状态，无法执行 {action}")]
    InvalidTransition {
        /// 当前状态的固定领域代码。
        from: &'static str,
        /// 请求执行的固定动作代码。
        action: &'static str,
    },
    /// 请求的版本与当前挂单版本不同。
    #[error("挂单版本已变化，预期 {expected}，当前 {actual}")]
    StaleVersion {
        /// 命令预期的版本。
        expected: u64,
        /// 当前挂单版本。
        actual: u64,
    },
    /// 卖家不具备操作所需的资格。
    #[error("卖家不具备挂单资格")]
    Ineligible,
    /// 装备已被有效挂单占用或仍有未结清的拍卖责任。
    #[error("装备已被占用或仍有未结清的拍卖责任")]
    EquipmentBusy,
    /// 本站历史缺少足以确认最后持有人的证据。
    #[error("本站缺少足够的装备交付或返还证明")]
    HistoryUnproven,
    /// 本站已证的最后持有人与登记人不同。
    #[error("本站最后已证持有人与登记人不符")]
    HistoryOwnerMismatch,
    /// 同件装备的历史别名记录存在证据冲突。
    #[error("本站同件装备的历史记录互相矛盾")]
    HistoryAliasConflict,
    /// 同一卖家已有同件装备的活动挂单。
    #[error("卖家已有该装备的活动挂单")]
    AlreadyOpen,
}

/// 挂单终态原因的闭集，稳定名称用于数据库和 DTO。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListingReason {
    /// 装备不可交易。
    NotTradeable,
    /// 解析所得的当前持有人与登记人不符。
    OwnerMismatch,
    /// 本站最后已证持有人与登记人不符。
    HistoryOwnerMismatch,
    /// 装备历史别名的证据相互冲突。
    HistoryAliasConflict,
    /// 装备历史缺少足够证明。
    HistoryUnproven,
    /// 装备已被占用或仍有未结清的拍卖责任。
    EquipmentBusy,
    /// 来源找不到装备或读取凭证不匹配。
    EquipmentNotFound,
    /// 来源拒绝请求。
    SourceRequestRejected,
    /// 来源返回不支持的 HTTP 响应。
    SourceHttpInvalid,
    /// 来源响应不符合结构要求。
    SourceSchemaInvalid,
    /// 来源响应超出读取预算。
    SourceTooLarge,
    /// 来源返回尚未确认的字段形态。
    SourceShapeUnconfirmed,
    /// 来源回显的装备身份与请求不符。
    SourceIdentityMismatch,
    /// 来源未提供完整的持有人信息。
    OwnerMissing,
    /// 登记凭证不可用。
    CredentialUnavailable,
    /// 解析在总期限内耗尽尝试次数。
    LookupExhausted,
    /// 解析期限已到。
    LookupDeadline,
    /// 展示期限已到。
    DisplayExpired,
    /// 待寄期限已到。
    DepositExpired,
    /// 已提交的拍卖 lot 不在登记基线中。
    AuctionLotCreated,
    /// 卖家已失去资格或出现登记后的新拉黑事实。
    SellerIneligible,
    /// 寄售服务已停用。
    ServiceDisabled,
    /// 卖家主动取消挂单。
    SellerCancelled,
    /// 管理员下架挂单。
    AdminDelisted,
}

/// 挂单终态原因的字符串不在闭集中。
#[derive(Debug, Error, PartialEq, Eq)]
#[error("未知的挂单终态原因码")]
pub struct UnknownListingReason;

impl ListingReason {
    const ALL: [Self; 24] = [
        Self::NotTradeable,
        Self::OwnerMismatch,
        Self::HistoryOwnerMismatch,
        Self::HistoryAliasConflict,
        Self::HistoryUnproven,
        Self::EquipmentBusy,
        Self::EquipmentNotFound,
        Self::SourceRequestRejected,
        Self::SourceHttpInvalid,
        Self::SourceSchemaInvalid,
        Self::SourceTooLarge,
        Self::SourceShapeUnconfirmed,
        Self::SourceIdentityMismatch,
        Self::OwnerMissing,
        Self::CredentialUnavailable,
        Self::LookupExhausted,
        Self::LookupDeadline,
        Self::DisplayExpired,
        Self::DepositExpired,
        Self::AuctionLotCreated,
        Self::SellerIneligible,
        Self::ServiceDisabled,
        Self::SellerCancelled,
        Self::AdminDelisted,
    ];

    /// 返回数据库和 DTO 共用的稳定原因码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotTradeable => "not_tradeable",
            Self::OwnerMismatch => "owner_mismatch",
            Self::HistoryOwnerMismatch => "history_owner_mismatch",
            Self::HistoryAliasConflict => "history_alias_conflict",
            Self::HistoryUnproven => "history_unproven",
            Self::EquipmentBusy => "equipment_busy",
            Self::EquipmentNotFound => "equipment_not_found",
            Self::SourceRequestRejected => "source_request_rejected",
            Self::SourceHttpInvalid => "source_http_invalid",
            Self::SourceSchemaInvalid => "source_schema_invalid",
            Self::SourceTooLarge => "source_too_large",
            Self::SourceShapeUnconfirmed => "source_shape_unconfirmed",
            Self::SourceIdentityMismatch => "source_identity_mismatch",
            Self::OwnerMissing => "owner_missing",
            Self::CredentialUnavailable => "credential_unavailable",
            Self::LookupExhausted => "lookup_exhausted",
            Self::LookupDeadline => "lookup_deadline",
            Self::DisplayExpired => "display_expired",
            Self::DepositExpired => "deposit_expired",
            Self::AuctionLotCreated => "auction_lot_created",
            Self::SellerIneligible => "seller_ineligible",
            Self::ServiceDisabled => "service_disabled",
            Self::SellerCancelled => "seller_cancelled",
            Self::AdminDelisted => "admin_delisted",
        }
    }

    /// 精确解析稳定原因码，未知值失败关闭且错误不回显输入。
    pub fn parse(value: &str) -> Result<Self, UnknownListingReason> {
        // 不变量 6：暂时性来源错误属于工作进度，只有耗尽才写入挂单终态原因。
        Self::ALL.into_iter().find(|reason| reason.as_str() == value).ok_or(UnknownListingReason)
    }

    /// 返回该原因允许对应的终态稳定代码。
    pub const fn terminal_state(self) -> &'static str {
        // 不变量 6：每个原因固定对应一个终态，避免接收不合法的状态和原因组合。
        match self {
            Self::NotTradeable
            | Self::OwnerMismatch
            | Self::HistoryOwnerMismatch
            | Self::HistoryAliasConflict
            | Self::HistoryUnproven
            | Self::EquipmentBusy
            | Self::EquipmentNotFound
            | Self::SourceRequestRejected
            | Self::SourceHttpInvalid
            | Self::SourceSchemaInvalid
            | Self::SourceTooLarge
            | Self::SourceShapeUnconfirmed
            | Self::SourceIdentityMismatch
            | Self::OwnerMissing
            | Self::CredentialUnavailable
            | Self::LookupExhausted => "rejected",
            Self::LookupDeadline | Self::DisplayExpired | Self::DepositExpired => "expired",
            Self::AuctionLotCreated
            | Self::SellerIneligible
            | Self::ServiceDisabled
            | Self::SellerCancelled
            | Self::AdminDelisted => "closed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ListingError, ListingReason, UnknownListingReason};

    // 期望值来自合同第 2.1 节和第 5.1 节。
    const REASON_CASES: [(ListingReason, &str, &str); 24] = [
        (ListingReason::NotTradeable, "not_tradeable", "rejected"),
        (ListingReason::OwnerMismatch, "owner_mismatch", "rejected"),
        (ListingReason::HistoryOwnerMismatch, "history_owner_mismatch", "rejected"),
        (ListingReason::HistoryAliasConflict, "history_alias_conflict", "rejected"),
        (ListingReason::HistoryUnproven, "history_unproven", "rejected"),
        (ListingReason::EquipmentBusy, "equipment_busy", "rejected"),
        (ListingReason::EquipmentNotFound, "equipment_not_found", "rejected"),
        (ListingReason::SourceRequestRejected, "source_request_rejected", "rejected"),
        (ListingReason::SourceHttpInvalid, "source_http_invalid", "rejected"),
        (ListingReason::SourceSchemaInvalid, "source_schema_invalid", "rejected"),
        (ListingReason::SourceTooLarge, "source_too_large", "rejected"),
        (ListingReason::SourceShapeUnconfirmed, "source_shape_unconfirmed", "rejected"),
        (ListingReason::SourceIdentityMismatch, "source_identity_mismatch", "rejected"),
        (ListingReason::OwnerMissing, "owner_missing", "rejected"),
        (ListingReason::CredentialUnavailable, "credential_unavailable", "rejected"),
        (ListingReason::LookupExhausted, "lookup_exhausted", "rejected"),
        (ListingReason::LookupDeadline, "lookup_deadline", "expired"),
        (ListingReason::DisplayExpired, "display_expired", "expired"),
        (ListingReason::DepositExpired, "deposit_expired", "expired"),
        (ListingReason::AuctionLotCreated, "auction_lot_created", "closed"),
        (ListingReason::SellerIneligible, "seller_ineligible", "closed"),
        (ListingReason::ServiceDisabled, "service_disabled", "closed"),
        (ListingReason::SellerCancelled, "seller_cancelled", "closed"),
        (ListingReason::AdminDelisted, "admin_delisted", "closed"),
    ];

    #[test]
    fn listing_reason_codes_round_trip() {
        for (reason, code, _) in REASON_CASES {
            assert_eq!(reason.as_str(), code, "{reason:?}");
            assert_eq!(ListingReason::parse(code), Ok(reason), "{code}");
        }
    }

    #[test]
    fn listing_reason_parse_rejects_unknown_and_nonterminal_codes() {
        for code in [
            "unknown_reason",
            "",
            "Not_Tradeable",
            "not_tradeable ",
            "repriced",
            "already_open",
            "source_unavailable",
            "source_timeout",
            "source_rate_limited",
        ] {
            assert_eq!(ListingReason::parse(code), Err(UnknownListingReason));
        }
        assert_eq!(UnknownListingReason.to_string(), "未知的挂单终态原因码");
    }

    #[test]
    fn listing_reasons_match_contract_terminal_states() {
        for (reason, code, terminal_state) in REASON_CASES {
            assert_eq!(reason.terminal_state(), terminal_state, "{code}");
        }
    }

    #[test]
    fn listing_error_messages_render_without_placeholders() {
        let cases = [
            (ListingError::InvalidIdentity, "装备身份无效"),
            (ListingError::InvalidPrice, "挂单标价无效"),
            (ListingError::InvalidFee, "挂单费率无效"),
            (
                ListingError::InvalidTransition { from: "closed", action: "reprice" },
                "挂单处于 closed 状态，无法执行 reprice",
            ),
            (
                ListingError::StaleVersion { expected: 2, actual: 3 },
                "挂单版本已变化，预期 2，当前 3",
            ),
            (ListingError::Ineligible, "卖家不具备挂单资格"),
            (ListingError::EquipmentBusy, "装备已被占用或仍有未结清的拍卖责任"),
            (ListingError::HistoryUnproven, "本站缺少足够的装备交付或返还证明"),
            (ListingError::HistoryOwnerMismatch, "本站最后已证持有人与登记人不符"),
            (ListingError::HistoryAliasConflict, "本站同件装备的历史记录互相矛盾"),
            (ListingError::AlreadyOpen, "卖家已有该装备的活动挂单"),
        ];
        for (error, message) in cases {
            assert_eq!(error.to_string(), message, "{error:?}");
        }
    }
}
