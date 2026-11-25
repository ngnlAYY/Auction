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
    /// 卖家重列：把已止售的原件重新上架，新增 g+1 处分代（第 7 期）。
    Relist,
    /// Admin 确认代管件灭失（第 7 期）。
    ConfirmPositionLoss,
    /// Admin 追加损失案件的人工结果（第 7 期）。
    RecordLossCaseOutcome,
    /// 系统追加灭失证据（第 7 期）。
    RecordPositionLossProof,
    /// 系统履行取得之前登记的撤回意愿（第 7 期）。
    FulfillWithdrawIntent,
}

impl DepositOperation {
    /// 全部操作；wire 与持久账本闭集必须逐项对应。
    pub const ALL: [Self; 18] = [
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
        Self::Relist,
        Self::ConfirmPositionLoss,
        Self::RecordLossCaseOutcome,
        Self::RecordPositionLossProof,
        Self::FulfillWithdrawIntent,
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
            Self::Relist => "relist",
            Self::ConfirmPositionLoss => "confirm_position_loss",
            Self::RecordLossCaseOutcome => "record_loss_case_outcome",
            Self::RecordPositionLossProof => "record_position_loss_proof",
            Self::FulfillWithdrawIntent => "fulfill_withdraw_intent",
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
    /// 直接撤回已被接受，合同与装备已绑定（第 7 期）。
    WithdrawalAccepted,
    /// 重列完成，新处分代已上架（第 7 期）。
    Relisted,
    /// 代管件灭失已按 position 办结（第 7 期）。
    PositionLossRecorded,
    /// 损失案件的人工结果已追加（第 7 期）。
    CaseOutcomeRecorded,
    /// 灭失证据已追加（第 7 期）。
    LossProofRecorded,
    /// 可信寄售 RTS 已被接管：intake、唯一 Take 工作与终局同提交（整封首次决定的接管分支）。
    ReturnAcquired,
}

impl ReceiptDisposition {
    /// 全部成功处置；顺序为本闭集的稳定声明顺序，取值与 `c4_valid_v2_receipt` 的
    /// CASE 分支逐字一致。
    pub const ALL: [Self; 25] = [
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
        Self::WithdrawalAccepted,
        Self::Relisted,
        Self::PositionLossRecorded,
        Self::CaseOutcomeRecorded,
        Self::LossProofRecorded,
        Self::ReturnAcquired,
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
            Self::WithdrawalAccepted => "withdrawal_accepted",
            Self::Relisted => "relisted",
            Self::PositionLossRecorded => "position_loss_recorded",
            Self::CaseOutcomeRecorded => "case_outcome_recorded",
            Self::LossProofRecorded => "loss_proof_recorded",
            Self::ReturnAcquired => "return_acquired",
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
    /// 受限权利流程里的原浏览器；稳定引用是浏览器绑定摘要的小写十六进制。
    Browser {
        /// `hv_right_browser` 随机量的 SHA-256 摘要。
        binding_digest: [u8; 32],
    },
    /// 已验证的受限权利人；稳定引用是 `right_id`。
    RightHolder {
        /// 权利 UUID。
        right_id: Uuid,
    },
}

impl DepositActor {
    /// 规范编码使用的 actor 类型码。
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::PublicUser(_) => "public_user",
            Self::Admin(_) => "admin",
            Self::Worker => "worker",
            Self::System => "system",
            Self::Browser { .. } => "browser",
            Self::RightHolder { .. } => "right_holder",
        }
    }

    /// 系统与服务引用的固定文本。
    pub const DEPOSIT_SERVICE_REF: &'static str = "consignment";
}

#[cfg(test)]
mod tests {
    use super::{DepositOperation, ReceiptDisposition};

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
        let actual = ReceiptDisposition::ALL[..19].iter().map(|item| item.as_str());
        assert!(actual.eq(expected));
    }

    /// 第 7 期新增的处置码与操作码是 7M-2 要放进 000137 `c4_valid_v2_receipt` 与账本闭集的
    /// 逐字值；在迁移合入前由本测试钉住 wire 码，改名必须同时改迁移。
    #[test]
    fn phase_seven_wire_codes_are_frozen() {
        let dispositions =
            ReceiptDisposition::ALL[19..].iter().map(|item| item.as_str()).collect::<Vec<_>>();
        assert_eq!(
            dispositions,
            [
                "withdrawal_accepted",
                "relisted",
                "position_loss_recorded",
                "case_outcome_recorded",
                "loss_proof_recorded",
                "return_acquired",
            ]
        );
        let operations =
            DepositOperation::ALL[13..].iter().map(|item| item.as_str()).collect::<Vec<_>>();
        assert_eq!(
            operations,
            [
                "relist",
                "confirm_position_loss",
                "record_loss_case_outcome",
                "record_position_loss_proof",
                "fulfill_withdraw_intent",
            ]
        );
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
