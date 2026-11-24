//! 寄售收件与代管的共同命令语言：操作闭集、actor 与回执版本。

use auction_domain::UserId;
use uuid::Uuid;

/// 第 4 期 v2 回执的固定版本。
pub const DEPOSIT_SCHEMA_VERSION: u16 = 2;

/// 第 4 期新增操作的稳定闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DepositOperation {
    /// 签发或换发预约。
    IssueReservation,
    /// 退休预约。
    ExpireReservation,
    /// 首次整封决定。
    DecideEnvelope,
    /// 追加隔离观察。
    AppendQuarantineObservation,
    /// 排队隔离重检。
    RecheckQuarantine,
    /// 解除隔离。
    ReleaseQuarantine,
    /// 提交一次资产外写起写。
    StartDispositionWrite,
    /// 应用邮件证明。
    ApplyMailProof,
    /// 卖家登记撤回意愿。
    Withdraw,
    /// Admin 下架代管件或未代管件。
    AdminDelist,
    /// 解除指定运营止售。
    ReleaseOperationsHold,
    /// 季前撤回通知。
    SeasonNotice,
    /// 关闭旧季。
    SeasonClose,
}

impl DepositOperation {
    /// 全部操作；wire 与持久账本闭集必须逐项对应。
    pub const ALL: [Self; 13] = [
        Self::IssueReservation,
        Self::ExpireReservation,
        Self::DecideEnvelope,
        Self::AppendQuarantineObservation,
        Self::RecheckQuarantine,
        Self::ReleaseQuarantine,
        Self::StartDispositionWrite,
        Self::ApplyMailProof,
        Self::Withdraw,
        Self::AdminDelist,
        Self::ReleaseOperationsHold,
        Self::SeasonNotice,
        Self::SeasonClose,
    ];

    /// 稳定 wire 与账本码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IssueReservation => "issue_reservation",
            Self::ExpireReservation => "expire_reservation",
            Self::DecideEnvelope => "decide_envelope",
            Self::AppendQuarantineObservation => "append_quarantine_observation",
            Self::RecheckQuarantine => "recheck_quarantine",
            Self::ReleaseQuarantine => "release_quarantine",
            Self::StartDispositionWrite => "start_disposition_write",
            Self::ApplyMailProof => "apply_mail_proof",
            Self::Withdraw => "withdraw",
            Self::AdminDelist => "admin_delist",
            Self::ReleaseOperationsHold => "release_hold",
            Self::SeasonNotice => "season_notice",
            Self::SeasonClose => "season_close",
        }
    }

    /// 闭集解析，未知值失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|operation| operation.as_str() == value)
    }
}

/// v2 回执的成功处置闭集。
///
/// 逐字对齐 `migrations/postgres/000134_consignment_deposit_custody.up.sql` 的
/// `c4_valid_v2_receipt`：公开处置由 HTTP 回执返回，内部处置只进持久账本，不对外。
/// 失败回执的 `rejected` 由共同错误路径表达，不在此成功处置闭集内。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReceiptDisposition {
    /// 预约已签发。
    ReservationIssued,
    /// 已登记撤回意愿。
    WithdrawalRequested,
    /// 未代管件已关闭。
    Closed,
    /// 运营止售已设置。
    HoldSet,
    /// 运营止售已解除。
    HoldReleased,
    /// 隔离重查工作已排入。
    RecheckQueued,
    /// 隔离已解除并交给唯一 owner。
    QuarantineReleased,
    /// 隔离观察已追加。
    ObservationAppended,
    /// 预约已退休。
    ReservationRetired,
    /// 整封接受。
    Accepted,
    /// 整封退回。
    ReturnDecided,
    /// 空信归档。
    ArchivedEmpty,
    /// 持久隔离。
    Quarantined,
    /// 资产起写已提交。
    WriteStarted,
    /// 取得已证明。
    TakeProven,
    /// 退回已证明。
    ReturnProven,
    /// 证据不足或冲突，转隔离或核对。
    EvidenceQuarantined,
    /// 季前通知已排队。
    NoticeQueued,
    /// 旧季已关闭。
    SeasonClosed,
}

impl ReceiptDisposition {
    /// 全部成功处置；顺序为本闭集的稳定声明顺序，取值与 `c4_valid_v2_receipt` 的
    /// CASE 分支逐字一致。
    pub const ALL: [Self; 19] = [
        Self::ReservationIssued,
        Self::WithdrawalRequested,
        Self::Closed,
        Self::HoldSet,
        Self::HoldReleased,
        Self::RecheckQueued,
        Self::QuarantineReleased,
        Self::ObservationAppended,
        Self::ReservationRetired,
        Self::Accepted,
        Self::ReturnDecided,
        Self::ArchivedEmpty,
        Self::Quarantined,
        Self::WriteStarted,
        Self::TakeProven,
        Self::ReturnProven,
        Self::EvidenceQuarantined,
        Self::NoticeQueued,
        Self::SeasonClosed,
    ];

    /// 稳定 wire 与账本码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReservationIssued => "reservation_issued",
            Self::WithdrawalRequested => "withdrawal_requested",
            Self::Closed => "closed",
            Self::HoldSet => "hold_set",
            Self::HoldReleased => "hold_released",
            Self::RecheckQueued => "recheck_queued",
            Self::QuarantineReleased => "quarantine_released",
            Self::ObservationAppended => "observation_appended",
            Self::ReservationRetired => "reservation_retired",
            Self::Accepted => "accepted",
            Self::ReturnDecided => "return_decided",
            Self::ArchivedEmpty => "archived_empty",
            Self::Quarantined => "quarantined",
            Self::WriteStarted => "write_started",
            Self::TakeProven => "take_proven",
            Self::ReturnProven => "return_proven",
            Self::EvidenceQuarantined => "evidence_quarantined",
            Self::NoticeQueued => "notice_queued",
            Self::SeasonClosed => "season_closed",
        }
    }
}

/// 命令 actor 的稳定分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepositActor {
    /// 已认证玩家；稳定引用使用用户 UUID。
    PublicUser(UserId),
    /// 已认证 Admin；稳定引用使用认证文本。
    Admin(String),
    /// Worker 派生的系统命令。
    Worker,
    /// 自动复判等系统变体。
    System,
}

impl DepositActor {
    /// 规范编码使用的 actor 类型码。
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::PublicUser(_) => "public_user",
            Self::Admin(_) => "admin",
            Self::Worker => "worker",
            Self::System => "system",
        }
    }

    /// 系统与服务引用的固定文本。
    pub const DEPOSIT_SERVICE_REF: &'static str = "consignment";
}

#[cfg(test)]
mod tests {
    use super::ReceiptDisposition;

    /// 000134 `c4_valid_v2_receipt` 的每个成功分支都必须能由 4A 的闭集命名；新增分支只能
    /// 同时扩这两处，避免账本与领域再次分叉。
    #[test]
    fn receipt_dispositions_match_the_database_receipt_guard() {
        let expected = [
            "reservation_issued",
            "withdrawal_requested",
            "closed",
            "hold_set",
            "hold_released",
            "recheck_queued",
            "quarantine_released",
            "observation_appended",
            "reservation_retired",
            "accepted",
            "return_decided",
            "archived_empty",
            "quarantined",
            "write_started",
            "take_proven",
            "return_proven",
            "evidence_quarantined",
            "notice_queued",
            "season_closed",
        ];
        let actual = ReceiptDisposition::ALL.map(ReceiptDisposition::as_str);
        assert_eq!(actual, expected);
    }
}

/// 挂单在成功回执中的最小变化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingStateChange {
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 提交后的版本。
    pub listing_version: u64,
    /// 提交后的持久状态。
    pub state: auction_domain::consignment::ListingState,
}
