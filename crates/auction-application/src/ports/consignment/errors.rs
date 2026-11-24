//! 寄售写入决定、失败边界及安全错误码。

use auction_domain::consignment::ListingError;
use auction_domain::consignment::deposit::DepositError;
use thiserror::Error;

use super::deposit::ReservationSecretCryptoError;
use crate::errors::ApplicationError;
use crate::ports::core::{RepositoryError, TransactionFuture};

/// 寄售端口的装箱异步结果，保留本上下文的错误分类。
pub type ConsignmentFuture<'a, T> = TransactionFuture<'a, T, ConsignmentError>;

/// 可持久化和重放的确定业务拒绝，数据只包含安全分类。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ListingBusinessRejection {
    /// 目标不存在或不属于当前卖家。
    #[error("挂单不存在")]
    ListingNotFound,
    /// 卖家不具备挂单资格。
    #[error("卖家不具备挂单资格")]
    ParticipantIneligible,
    /// 同一卖家的同件装备已有活动登记。
    #[error("同件装备已有活动登记")]
    AlreadyOpen,
    /// 装备已被占用或仍有拍卖责任。
    #[error("装备已被占用或仍有拍卖责任")]
    EquipmentBusy,
    /// 无法证明本站最后持有人。
    #[error("本站装备历史证据不足")]
    HistoryUnproven,
    /// 本站已证持有人与登记人不符。
    #[error("本站最后已证持有人与登记人不符")]
    HistoryOwnerMismatch,
    /// 同件装备的本站历史证据相互矛盾。
    #[error("本站同件装备的历史记录互相矛盾")]
    HistoryAliasConflict,
    /// 适用费率与登记确认时不同。
    #[error("挂单费率已变化")]
    FeeChanged,
    /// 预期版本与当前版本不同。
    #[error("挂单版本已变化")]
    StaleVersion,
    /// 挂单已停止接受该操作。
    #[error("挂单已停止接受操作")]
    ListingNotActive,
    /// 该世界不接收新登记。
    #[error("该世界暂不接受新登记")]
    WorldUnavailable,
    /// 滚动小时登记额度已满。
    #[error("本小时登记次数已达上限")]
    RegistrationRateLimited,
    /// 两世界有效活动登记额度已满。
    #[error("活动挂单数已达上限")]
    ListingLimitReached,
}

/// 寄售第 4 期收件与代管写入的确定业务拒绝闭集。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DepositBusinessRejection {
    /// 版本已变化。
    #[error("寄售版本已变化")]
    StaleVersion,
    /// 挂单已不接受该动作。
    #[error("寄售挂单已停止接受该动作")]
    ListingNotActive,
    /// 装备已被占用或仍有未结清责任。
    #[error("装备已被占用或仍有未结清责任")]
    EquipmentBusy,
    /// 卖家或受限权利人不具备资格。
    #[error("参与者不具备寄售资格")]
    ParticipantIneligible,
    /// 认证 Bot 不能作为交易当事人。
    #[error("认证 Bot 不能作为寄售交易当事人")]
    BotParty,
    /// 旧季已关闭或季界无法判定。
    #[error("寄售季已关闭")]
    SeasonClosed,
    /// 邮箱或证明能力确定不可用。
    #[error("寄售收件能力暂不可用")]
    MailUnavailable,
    /// 来源与操作不匹配。
    #[error("来源与本操作不匹配")]
    SourceMismatch,
    /// 缺少完成动作所需的证明。
    #[error("缺少完成动作所需的证明")]
    EvidenceUnproven,
    /// 可靠证据互相矛盾。
    #[error("可靠证据互相矛盾")]
    EvidenceConflict,
    /// 已存在相反或不同的最终处置。
    #[error("已存在相反的最终处置")]
    DispositionConflict,
    /// 指定止售与当前有效止售不一致。
    #[error("运营止售不匹配")]
    HoldMismatch,
    /// 当前状态、角色或动作不允许。
    #[error("当前状态或角色不允许该动作")]
    ActionNotAllowed,
    /// 起写 fence 已失效。
    #[error("资产起写 fence 不匹配")]
    FenceMismatch,
}

impl DepositBusinessRejection {
    /// 返回账本和 API 使用的固定业务码。
    pub const fn code(self) -> &'static str {
        match self {
            Self::StaleVersion => "stale_version",
            Self::ListingNotActive => "listing_not_active",
            Self::EquipmentBusy => "equipment_busy",
            Self::ParticipantIneligible => "participant_ineligible",
            Self::BotParty => "bot_party",
            Self::SeasonClosed => "season_closed",
            Self::MailUnavailable => "mail_unavailable",
            Self::SourceMismatch => "source_mismatch",
            Self::EvidenceUnproven => "evidence_unproven",
            Self::EvidenceConflict => "evidence_conflict",
            Self::DispositionConflict => "disposition_conflict",
            Self::HoldMismatch => "hold_mismatch",
            Self::ActionNotAllowed => "action_not_allowed",
            Self::FenceMismatch => "fence_mismatch",
        }
    }

    /// 返回固定 HTTP 状态。
    pub const fn status(self) -> u16 {
        match self {
            Self::ParticipantIneligible | Self::BotParty | Self::ActionNotAllowed => 403,
            Self::MailUnavailable => 503,
            Self::StaleVersion
            | Self::ListingNotActive
            | Self::EquipmentBusy
            | Self::SeasonClosed
            | Self::SourceMismatch
            | Self::EvidenceUnproven
            | Self::EvidenceConflict
            | Self::DispositionConflict
            | Self::HoldMismatch
            | Self::FenceMismatch => 409,
        }
    }
}

impl ListingBusinessRejection {
    /// 返回账本和 API 使用的固定业务码。
    pub const fn code(self) -> &'static str {
        match self {
            Self::ListingNotFound => "listing_not_found",
            Self::ParticipantIneligible => "participant_ineligible",
            Self::AlreadyOpen => "already_open",
            Self::EquipmentBusy => "equipment_busy",
            Self::HistoryUnproven => "history_unproven",
            Self::HistoryOwnerMismatch => "history_owner_mismatch",
            Self::HistoryAliasConflict => "history_alias_conflict",
            Self::FeeChanged => "fee_changed",
            Self::StaleVersion => "stale_version",
            Self::ListingNotActive => "listing_not_active",
            Self::WorldUnavailable => "world_unavailable",
            Self::RegistrationRateLimited => "registration_rate_limited",
            Self::ListingLimitReached => "listing_limit_reached",
        }
    }

    /// 返回初次写入决定的状态码，查账本时保留该值。
    pub const fn status(self) -> u16 {
        match self {
            Self::ListingNotFound => 404,
            Self::ParticipantIneligible => 403,
            Self::RegistrationRateLimited | Self::ListingLimitReached => 429,
            Self::AlreadyOpen
            | Self::EquipmentBusy
            | Self::HistoryUnproven
            | Self::HistoryOwnerMismatch
            | Self::HistoryAliasConflict
            | Self::FeeChanged
            | Self::StaleVersion
            | Self::ListingNotActive
            | Self::WorldUnavailable => 409,
        }
    }
}

/// 不产生新账本行的确定性仲裁结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnrecordedListingRejection {
    /// 该命令 ID 已绑定其他请求，不返回原决定。
    CommandIdConflict,
    /// 未受理的新登记或本人改价遇到关闭门。
    Disabled,
    /// 首次解析提交的工作围栏或租约已失效。
    LeaseLost,
    /// 自动下架尚未观察到可证明的条件，保留以后受理机会。
    ConditionNotMet,
}

/// 寄售用例对外的安全错误，不保留依赖错误链或输入原文。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ConsignmentError {
    /// 已确定的安全业务拒绝。
    #[error(transparent)]
    Business(#[from] ListingBusinessRejection),
    /// 寄售收件与代管的确定业务拒绝。
    #[error(transparent)]
    Deposit(#[from] DepositBusinessRejection),
    /// 同一命令 ID 对应不同请求。
    #[error("命令编号已用于不同请求")]
    CommandIdConflict,
    /// 新业务当前关闭。
    #[error("一口价服务已关闭")]
    Disabled,
    /// 命令 UUID 无效。
    #[error("命令编号无效")]
    InvalidCommandId,
    /// 路径中的挂单 UUID 无效。
    #[error("挂单编号无效")]
    InvalidListingId,
    /// 正文、预期版本或费率的输入无效。
    #[error("挂单请求无效")]
    InvalidListingRequest,
    /// 寄售收件、隔离或代管请求的输入无效。
    #[error("寄售收件请求无效")]
    InvalidDepositRequest,
    /// 装备 URL 或其身份、读取凭证无效。
    #[error("装备链接无效")]
    InvalidEquipmentUrl,
    /// 标价无效。
    #[error("挂单标价无效")]
    InvalidPrice,
    /// 运营下架说明无效。
    #[error("下架说明无效")]
    InvalidReason,
    /// 查询筛选或分页不符合约束。
    #[error("挂单查询参数无效")]
    InvalidQuery,
    /// 权威账本没有该演员可见的命令。
    #[error("命令回执不存在")]
    CommandNotFound,
    /// 存储、加密或 Worker 能力不足，或事务重试耗尽。
    #[error("一口价服务暂不可用")]
    Unavailable,
    /// 无法确认提交是否成功，调用方只能查原回执或原样重试。
    #[error("无法确认挂单命令结果")]
    OutcomeUnknown,
    /// 围栏失效，供 Worker 内部处理。
    #[error("解析租约已失效")]
    LeaseLost,
    /// 缺少可核实的强制清退条件，供 Worker 内部处理。
    #[error("强制清退条件尚未成立")]
    ConditionNotMet,
}

impl ConsignmentError {
    /// API 可见的状态码及固定错误码。内部 Worker 拒绝没有 API 映射。
    pub const fn api_error(self) -> Option<(u16, &'static str)> {
        Some(match self {
            Self::Business(error) => (error.status(), error.code()),
            Self::Deposit(error) => (error.status(), error.code()),
            Self::CommandIdConflict => (409, "command_id_conflict"),
            Self::Disabled => (404, "consignment_disabled"),
            Self::InvalidCommandId => (400, "invalid_command_id"),
            Self::InvalidListingId => (400, "invalid_listing_id"),
            Self::InvalidListingRequest => (400, "invalid_listing_request"),
            Self::InvalidDepositRequest => (400, "invalid_request"),
            Self::InvalidEquipmentUrl => (400, "invalid_equipment_url"),
            Self::InvalidPrice => (400, "invalid_price"),
            Self::InvalidReason => (400, "invalid_reason"),
            Self::InvalidQuery => (400, "invalid_query"),
            Self::CommandNotFound => (404, "command_not_found"),
            Self::Unavailable => (503, "consignment_unavailable"),
            Self::OutcomeUnknown => (503, "outcome_unknown"),
            Self::LeaseLost | Self::ConditionNotMet => return None,
        })
    }
}

impl From<DepositError> for ConsignmentError {
    fn from(error: DepositError) -> Self {
        use DepositBusinessRejection as Rejection;
        match error {
            DepositError::StaleVersion { .. } => Rejection::StaleVersion.into(),
            DepositError::ListingNotActive => Rejection::ListingNotActive.into(),
            DepositError::EquipmentBusy => Rejection::EquipmentBusy.into(),
            DepositError::SellerIneligible => Rejection::ParticipantIneligible.into(),
            DepositError::BotParty => Rejection::BotParty.into(),
            DepositError::SeasonClosed
            | DepositError::SeasonUnknown
            | DepositError::NotSeasonal => Rejection::SeasonClosed.into(),
            DepositError::MailUnavailable => Rejection::MailUnavailable.into(),
            DepositError::SourceMismatch => Rejection::SourceMismatch.into(),
            DepositError::EvidenceUnproven
            | DepositError::InvalidProof
            | DepositError::AcquireReturnUnavailable => Rejection::EvidenceUnproven.into(),
            DepositError::EvidenceConflict => Rejection::EvidenceConflict.into(),
            DepositError::DispositionConflict => Rejection::DispositionConflict.into(),
            DepositError::HoldMismatch => Rejection::HoldMismatch.into(),
            DepositError::ActionNotAllowed | DepositError::InvalidPositionState => {
                Rejection::ActionNotAllowed.into()
            }
            DepositError::FenceMismatch => Rejection::FenceMismatch.into(),
            DepositError::ReservationExpired
            | DepositError::ReservationNotActive
            | DepositError::ReservationMismatch
            | DepositError::ReservationCountMismatch => Rejection::EvidenceUnproven.into(),
            // 季窗来自日历配置，不是客户端输入；配置或日历错误按能力不可用处理。
            DepositError::InvalidSeasonWindow => Self::Unavailable,
            DepositError::TokenPrefix
            | DepositError::TokenLength
            | DepositError::TokenAlphabet
            | DepositError::TokenNonCanonical
            | DepositError::BodyEmpty
            | DepositError::BodyTooLarge
            | DepositError::TooManyItems
            | DepositError::BodyBlankLine
            | DepositError::BodyIsolatedCr
            | DepositError::BodyDuplicate
            | DepositError::BodyFreeText
            | DepositError::InvalidCalendar
            | DepositError::InvalidRequest => Self::InvalidDepositRequest,
        }
    }
}

impl From<ReservationSecretCryptoError> for ConsignmentError {
    fn from(_error: ReservationSecretCryptoError) -> Self {
        // 密码边界只暴露固定的不可用分类；底层的密钥、密文或认证失败细节不跨端口。
        Self::Unavailable
    }
}

impl From<ListingError> for ConsignmentError {
    fn from(error: ListingError) -> Self {
        match error {
            ListingError::InvalidIdentity => Self::InvalidEquipmentUrl,
            ListingError::InvalidPrice => Self::InvalidPrice,
            // 非法输入费率与已确认但不再适用的费率是两类错误。
            ListingError::InvalidFee => Self::InvalidListingRequest,
            ListingError::InvalidTransition { from, action } => invalid_transition(from, action),
            ListingError::StaleVersion { .. } => ListingBusinessRejection::StaleVersion.into(),
            ListingError::Ineligible => ListingBusinessRejection::ParticipantIneligible.into(),
            ListingError::EquipmentBusy => ListingBusinessRejection::EquipmentBusy.into(),
            ListingError::HistoryUnproven => ListingBusinessRejection::HistoryUnproven.into(),
            ListingError::HistoryOwnerMismatch => {
                ListingBusinessRejection::HistoryOwnerMismatch.into()
            }
            ListingError::HistoryAliasConflict => {
                ListingBusinessRejection::HistoryAliasConflict.into()
            }
            ListingError::AlreadyOpen => ListingBusinessRejection::AlreadyOpen.into(),
        }
    }
}

fn invalid_transition(from: &str, action: &str) -> ConsignmentError {
    // 此映射依赖领域 InvalidTransition 的状态与动作字符串，领域改动时须同步这里与测试。
    // 快照损坏、配置错误、时刻越界、版本耗尽及未知组合均失败关闭，不能记作业务拒绝。
    match (from, action) {
        ("unknown", "decode_version") => ConsignmentError::InvalidListingRequest,
        ("rejected" | "expired" | "closed", "reprice" | "withdraw" | "admin_delist") => {
            ListingBusinessRejection::ListingNotActive.into()
        }
        (
            "deposit_accepted" | "available" | "sale_pending" | "held_off_market"
            | "withdrawal_pending",
            "reprice" | "withdraw" | "admin_delist",
        ) => ListingBusinessRejection::ListingNotActive.into(),
        _ => ConsignmentError::Unavailable,
    }
}

impl From<UnrecordedListingRejection> for ConsignmentError {
    fn from(rejection: UnrecordedListingRejection) -> Self {
        match rejection {
            UnrecordedListingRejection::CommandIdConflict => Self::CommandIdConflict,
            UnrecordedListingRejection::Disabled => Self::Disabled,
            UnrecordedListingRejection::LeaseLost => Self::LeaseLost,
            UnrecordedListingRejection::ConditionNotMet => Self::ConditionNotMet,
        }
    }
}

impl ConsignmentError {
    /// 将既有只读依赖的错误压成安全分类。可能已经提交的写入应显式返回结果未知。
    pub fn read_dependency_failure(_error: ApplicationError) -> Self {
        Self::Unavailable
    }
}

impl From<RepositoryError> for ConsignmentError {
    fn from(_error: RepositoryError) -> Self {
        Self::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::ListingError;
    use auction_domain::consignment::deposit::DepositError;

    use super::ConsignmentError;

    #[test]
    fn invalid_season_window_is_service_unavailable() {
        assert_eq!(
            ConsignmentError::from(DepositError::InvalidSeasonWindow).api_error(),
            Some((503, "consignment_unavailable"))
        );
    }

    /// 代管态与 v1 mutation 的组合是确定拒绝，不能退化成可重试的 503；未知组合仍失败关闭。
    #[test]
    fn custody_managed_v1_mutations_are_listing_not_active() {
        for state in [
            "deposit_accepted",
            "available",
            "sale_pending",
            "held_off_market",
            "withdrawal_pending",
        ] {
            for action in ["reprice", "withdraw", "admin_delist"] {
                assert_eq!(
                    ConsignmentError::from(ListingError::InvalidTransition { from: state, action })
                        .api_error(),
                    Some((409, "listing_not_active")),
                    "{state} × {action}"
                );
            }
        }
        assert_eq!(
            ConsignmentError::from(ListingError::InvalidTransition {
                from: "deposit_accepted",
                action: "apply_lookup",
            })
            .api_error(),
            Some((503, "consignment_unavailable"))
        );
    }
}
