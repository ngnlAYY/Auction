//! 寄售模式、世界、状态、私有原因和卖家动作的线协议闭集。

use crate::dto::public::consignment::wire::string_enum;

string_enum! {
    /// 挂单创建后保持不变的模式。
    pub enum ListingMode {
        /// 卖家自行交割。
        Direct => "direct",
        /// 平台代管模式，当前阶段尚未收件。
        Consigned => "consigned",
    }
}

string_enum! {
    /// 寄售装备所在世界。
    pub enum ListingWorld {
        /// 主世界。
        Main => "main",
        /// 异世界。
        Isekai => "isekai",
    }
}

string_enum! {
    /// 公开目录可见的挂单状态。
    pub enum PublicListingState {
        /// 非寄售广告已展示。
        Displayed => "displayed",
        /// 寄售挂单等待装备。
        AwaitingDeposit => "awaiting_deposit",
    }
}

string_enum! {
    /// 只有本人与 Admin 投影能读取的完整挂单状态。
    pub enum ListingState {
        /// 解析中。
        Resolving => "resolving",
        /// 已展示。
        Displayed => "displayed",
        /// 等待寄来装备。
        AwaitingDeposit => "awaiting_deposit",
        /// 核验被拒。
        Rejected => "rejected",
        /// 有效期已到。
        Expired => "expired",
        /// 挂单已关闭。
        Closed => "closed",
    }
}

string_enum! {
    /// 终态的安全原因码，与领域的同名闭集分别定义。
    pub enum ListingReason {
        /// 装备已进入拍卖。
        AuctionLotCreated => "auction_lot_created",
        /// 卖家资格已失效。
        SellerIneligible => "seller_ineligible",
        /// 服务已停用。
        ServiceDisabled => "service_disabled",
        /// 卖家已取消。
        SellerCancelled => "seller_cancelled",
        /// 运营已下架。
        AdminDelisted => "admin_delisted",
        /// 解析期限已到。
        LookupDeadline => "lookup_deadline",
        /// 展示期限已到。
        DisplayExpired => "display_expired",
        /// 待寄期限已到。
        DepositExpired => "deposit_expired",
        /// 装备不可交易。
        NotTradeable => "not_tradeable",
        /// 来源缺少有效持有人。
        OwnerMissing => "owner_missing",
        /// 来源的持有人与登记人不符。
        OwnerMismatch => "owner_mismatch",
        /// 本站已证最后持有人与登记人不符。
        HistoryOwnerMismatch => "history_owner_mismatch",
        /// 同件装备的历史证据冲突。
        HistoryAliasConflict => "history_alias_conflict",
        /// 交付或返还证据不足。
        HistoryUnproven => "history_unproven",
        /// 已有挂单占用或未结拍卖责任。
        EquipmentBusy => "equipment_busy",
        /// 来源未找到装备或读取凭证无效。
        EquipmentNotFound => "equipment_not_found",
        /// 来源拒绝请求。
        SourceRequestRejected => "source_request_rejected",
        /// 来源 HTTP 响应不受支持。
        SourceHttpInvalid => "source_http_invalid",
        /// 来源数据格式错误。
        SourceSchemaInvalid => "source_schema_invalid",
        /// 来源数据形态尚未确认。
        SourceShapeUnconfirmed => "source_shape_unconfirmed",
        /// 来源装备标识不符。
        SourceIdentityMismatch => "source_identity_mismatch",
        /// 来源响应超出预算。
        SourceTooLarge => "source_too_large",
        /// 解析尝试次数耗尽。
        LookupExhausted => "lookup_exhausted",
        /// 本地装备读取凭证不可用。
        CredentialUnavailable => "credential_unavailable",
    }
}

string_enum! {
    /// 暂时读取失败的安全分类，可在解析进度和耗尽结果中显示。
    pub enum LookupReason {
        /// 暂时无法连接来源。
        SourceUnavailable => "source_unavailable",
        /// 读取超时。
        SourceTimeout => "source_timeout",
        /// 来源限流。
        SourceRateLimited => "source_rate_limited",
    }
}

string_enum! {
    /// 由服务端判定并随本人视图返回的卖家操作。
    pub enum ListingAction {
        /// 修改标价。
        Reprice => "reprice",
        /// 取消挂单。
        Withdraw => "withdraw",
    }
}

impl ListingState {
    /// 判断挂单状态是否仍处于解析或公开活动阶段。
    pub fn is_active(self) -> bool {
        matches!(self, Self::Resolving | Self::Displayed | Self::AwaitingDeposit)
    }

    pub(super) fn matches_mode(self, mode: ListingMode) -> bool {
        match self {
            Self::Displayed => mode == ListingMode::Direct,
            Self::AwaitingDeposit => mode == ListingMode::Consigned,
            _ => true,
        }
    }
}

impl ListingReason {
    // 到期原因保留前态，供已落库终态核对模式与快照。
    pub(super) fn expiry_source_state(self) -> Option<ListingState> {
        match self {
            Self::LookupDeadline => Some(ListingState::Resolving),
            Self::DisplayExpired => Some(ListingState::Displayed),
            Self::DepositExpired => Some(ListingState::AwaitingDeposit),
            _ => None,
        }
    }

    // 与领域 ListingReason 的终态归类及合同 §5.1 的快照规则一致。
    pub(super) fn terminal_state(self) -> ListingState {
        match self {
            Self::AuctionLotCreated
            | Self::SellerIneligible
            | Self::ServiceDisabled
            | Self::SellerCancelled
            | Self::AdminDelisted => ListingState::Closed,
            Self::LookupDeadline | Self::DisplayExpired | Self::DepositExpired => {
                ListingState::Expired
            }
            Self::NotTradeable
            | Self::OwnerMissing
            | Self::OwnerMismatch
            | Self::HistoryOwnerMismatch
            | Self::HistoryAliasConflict
            | Self::HistoryUnproven
            | Self::EquipmentBusy
            | Self::EquipmentNotFound
            | Self::SourceRequestRejected
            | Self::SourceHttpInvalid
            | Self::SourceSchemaInvalid
            | Self::SourceShapeUnconfirmed
            | Self::SourceIdentityMismatch
            | Self::SourceTooLarge
            | Self::LookupExhausted
            | Self::CredentialUnavailable => ListingState::Rejected,
        }
    }

    pub(super) fn rejected_with_snapshot(self) -> bool {
        matches!(
            self,
            Self::NotTradeable
                | Self::OwnerMismatch
                | Self::HistoryOwnerMismatch
                | Self::HistoryAliasConflict
                | Self::HistoryUnproven
                | Self::EquipmentBusy
        )
    }

    pub(super) fn is_forced_closure(self) -> bool {
        matches!(self, Self::AuctionLotCreated | Self::SellerIneligible | Self::ServiceDisabled)
    }
}
