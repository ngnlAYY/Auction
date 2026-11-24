//! 第 4 期 v2 共同回执字段与可重放的写操作拒绝码。

use crate::dto::public::consignment::wire_v2::string_enum;

string_enum! {
    /// v2 写操作中可由 HTTP 回执确认的 operation。
    pub enum ConsignmentOperationV2 {
        /// 本人签发寄件预约。
        IssueReservation => "issue_reservation",
        /// 本人撤回或取消。
        Withdraw => "withdraw",
        /// Admin 下架或设运营止售。
        AdminDelist => "admin_delist",
        /// Admin 解除运营止售。
        ReleaseHold => "release_hold",
        /// Admin 重查隔离。
        RecheckQuarantine => "recheck_quarantine",
        /// Admin 或系统解除隔离。
        ReleaseQuarantine => "release_quarantine",
    }
}

string_enum! {
    /// 第 4 期写操作初次提交时记录的公开处置。
    ///
    /// `observation_appended`、`reservation_retired`、`accepted` 等内部处置只进 4M 的
    /// 持久账本，不出现在公开回执 DTO。
    pub enum ConsignmentDispositionV2 {
        /// 预约已签发。
        ReservationIssued => "reservation_issued",
        /// 已登记撤回意愿，等待原物取得事务。
        WithdrawalRequested => "withdrawal_requested",
        /// 未代管件已按前三期规则关闭。
        Closed => "closed",
        /// 运营止售已设置。
        HoldSet => "hold_set",
        /// 运营止售已解除。
        HoldReleased => "hold_released",
        /// 隔离重查工作已排入。
        RecheckQueued => "recheck_queued",
        /// 隔离已解除并交给唯一 owner。
        QuarantineReleased => "quarantine_released",
    }
}

string_enum! {
    /// 第 4 期写操作可返回的确定拒绝码与结果未知码。
    ///
    /// `outcome_unknown` 只表示提交结果不可判；504、断线与解码失败没有可用码，
    /// 客户端同样必须保留原 actor、目标、输入与命令 ID 原样重放。
    pub enum WriteRejectionCodeV2 {
        /// 同命令 ID 的首次请求与原请求不同；冲突不写任何事实，因此不入账。
        CommandIdConflict => "command_id_conflict",
        /// 目标版本已过期。
        StaleVersion => "stale_version",
        /// 挂单已停止接收该动作。
        ListingNotActive => "listing_not_active",
        /// 装备被其他活动事实占用。
        EquipmentBusy => "equipment_busy",
        /// 装备已有不可撤回的承诺；第 4 期不产生，后续期使用。
        EquipmentCommitted => "equipment_committed",
        /// 代管取得或持有链证据不足；第 4 期不产生，后续期使用。
        CustodyUnproven => "custody_unproven",
        /// 运营止售持续有效；第 4 期不产生，后续期使用。
        OperationsHold => "operations_hold",
        /// 季界已关闭。
        SeasonClosed => "season_closed",
        /// 解除止售引用的 hold 与当前有效 hold 不符。
        HoldMismatch => "hold_mismatch",
        /// 拟定解除结果与锁内来源不符。
        SourceMismatch => "source_mismatch",
        /// 资产起写 fence 已失效。
        FenceMismatch => "fence_mismatch",
        /// 证据不足以证明拟定结果。
        EvidenceUnproven => "evidence_unproven",
        /// 新证据与已保存的可靠清单或强证明冲突。
        EvidenceConflict => "evidence_conflict",
        /// 相反处置或相反支付仍然开放。
        DispositionConflict => "disposition_conflict",
        /// 参与者不具备资格。
        ParticipantIneligible => "participant_ineligible",
        /// 认证 Bot 不能作为该交易当事人。
        BotParty => "bot_party",
        /// 当前操作不允许作用于该目标状态。
        ActionNotAllowed => "action_not_allowed",
        /// 活动挂单或登记额度已满。
        ListingLimitReached => "listing_limit_reached",
        /// 寄售首次禁用，不进入账本。
        ConsignmentDisabled => "consignment_disabled",
        /// 确定写前已知邮件能力不可用；仅白名单客户端可信。
        MailUnavailable => "mail_unavailable",
        /// 提交结果未知，只能原样重放并先查命令账本。
        OutcomeUnknown => "outcome_unknown",
    }
}

string_enum! {
    /// 写请求在进入命令账本前就被拒绝的请求错误码。
    ///
    /// 这些码表示请求形状、命令 ID、目标或查询本身无效，均可确定没有写入任何事实，客户端
    /// 不得以“结果未知”重放业务命令。`503 consignment_unavailable` 不在本闭集内：它可能是
    /// 提交前的能力不可用，也可能是提交结果未知，客户端只能按“结果未知，原样重放”处理。
    /// 数据库的 v2 命令账本不记录本闭集。
    pub enum RequestErrorCodeV2 {
        /// 通用请求形状错误。
        InvalidRequest => "invalid_request",
        /// 命令 ID 不是规范 UUIDv4。
        InvalidCommandId => "invalid_command_id",
        /// 目标挂单 ID 无效。
        InvalidListingId => "invalid_listing_id",
        /// 登记或改价请求字段无效。
        InvalidListingRequest => "invalid_listing_request",
        /// 装备 URL 无效或含秘密形状。
        InvalidEquipmentUrl => "invalid_equipment_url",
        /// 价格无效。
        InvalidPrice => "invalid_price",
        /// 私有原因无效。
        InvalidReason => "invalid_reason",
        /// 查询或筛选输入无效。
        InvalidQuery => "invalid_query",
        /// 回执查询没有找到该命令。
        CommandNotFound => "command_not_found",
    }
}

impl RequestErrorCodeV2 {
    /// 与该码配对的 HTTP 状态。
    pub const fn http_status(self) -> i64 {
        match self {
            Self::CommandNotFound => 404,
            Self::InvalidRequest
            | Self::InvalidCommandId
            | Self::InvalidListingId
            | Self::InvalidListingRequest
            | Self::InvalidEquipmentUrl
            | Self::InvalidPrice
            | Self::InvalidReason
            | Self::InvalidQuery => 400,
        }
    }

    /// 请求错误一律确定未写入，不得按未知结果重放业务命令。
    pub const fn is_replayable_as_unknown(self) -> bool {
        false
    }
}

impl WriteRejectionCodeV2 {
    /// 与该码配对的 HTTP 状态；`outcome_unknown` 固定为 503。
    pub const fn http_status(self) -> i64 {
        match self {
            Self::CommandIdConflict
            | Self::StaleVersion
            | Self::ListingNotActive
            | Self::EquipmentBusy
            | Self::EquipmentCommitted
            | Self::CustodyUnproven
            | Self::OperationsHold
            | Self::SeasonClosed
            | Self::HoldMismatch
            | Self::SourceMismatch
            | Self::FenceMismatch
            | Self::EvidenceUnproven
            | Self::EvidenceConflict
            | Self::DispositionConflict => 409,
            Self::ParticipantIneligible | Self::BotParty | Self::ActionNotAllowed => 403,
            Self::ListingLimitReached => 429,
            Self::ConsignmentDisabled => 404,
            Self::MailUnavailable | Self::OutcomeUnknown => 503,
        }
    }

    /// 只有该码要求按原输入重放并先查命令账本。
    pub const fn is_outcome_unknown(self) -> bool {
        matches!(self, Self::OutcomeUnknown)
    }

    /// 只有确定写前的 `mail_unavailable` 可在客户端白名单中视为可信 503。
    pub const fn is_trusted_before_write(self) -> bool {
        matches!(self, Self::MailUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::WriteRejectionCodeV2;

    #[test]
    fn rejection_status_classes_match_the_contract() {
        assert_eq!(WriteRejectionCodeV2::CommandIdConflict.http_status(), 409);
        assert_eq!(WriteRejectionCodeV2::ParticipantIneligible.http_status(), 403);
        assert_eq!(WriteRejectionCodeV2::ListingLimitReached.http_status(), 429);
        assert_eq!(WriteRejectionCodeV2::ConsignmentDisabled.http_status(), 404);
        assert_eq!(WriteRejectionCodeV2::MailUnavailable.http_status(), 503);
        assert_eq!(WriteRejectionCodeV2::FenceMismatch.http_status(), 409);
        assert!(WriteRejectionCodeV2::MailUnavailable.is_trusted_before_write());
        assert!(!WriteRejectionCodeV2::MailUnavailable.is_outcome_unknown());
        assert!(WriteRejectionCodeV2::OutcomeUnknown.is_outcome_unknown());
    }
}
