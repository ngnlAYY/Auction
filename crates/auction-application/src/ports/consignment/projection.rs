//! 本人与 Admin 读侧共用的有效状态和安全工作进度。

use auction_domain::consignment::{ListingMode, ListingReason, ListingState};
use auction_time::Timestamp;

use crate::ports::consignment::equipment::{
    ListingEquipmentDetails, ListingEquipmentLevel, LookupTransientReason,
};
use crate::ports::consignment::errors::ConsignmentError;
use crate::ports::consignment::work::MAX_LISTING_LOOKUP_ATTEMPTS;

/// 本人/Admin 共用的安全工作进度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingLookupProgress {
    /// 已提交的领取次数。
    attempts: u8,
    /// 排队时的下次可执行时刻。
    pub next_attempt_at: Option<Timestamp>,
    /// 最近一次暂时失败的安全分类。
    pub last_reason: Option<LookupTransientReason>,
    /// 当前外部访问暂停。
    pub paused: bool,
    /// 登记时冻结的解析总截止。
    pub deadline_at: Timestamp,
}

impl ListingLookupProgress {
    /// 从持久工作投影已领取次数。零代表尚无已提交的领取。
    pub fn new(
        attempts: u8,
        next_attempt_at: Option<Timestamp>,
        last_reason: Option<LookupTransientReason>,
        paused: bool,
        deadline_at: Timestamp,
    ) -> Result<Self, ConsignmentError> {
        if attempts > MAX_LISTING_LOOKUP_ATTEMPTS {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(Self { attempts, next_attempt_at, last_reason, paused, deadline_at })
    }

    /// 已提交的领取次数，范围 0–6。
    pub const fn attempts(self) -> u8 {
        self.attempts
    }
}

/// 有效态对应的私有字段，须经 `PrivateListingView::new` 核验后才能作为读模型。
#[derive(Debug, Clone, PartialEq)]
pub enum PrivateListingViewKind {
    /// 仍在解析，无公开快照。
    Resolving(ListingLookupProgress),
    /// 解析失败，无可信完整快照。
    LookupFailed {
        /// 挂单主拒绝原因。
        reason: ListingReason,
        /// 耗尽时保留的最后暂时分类。
        last_lookup_reason: Option<LookupTransientReason>,
    },
    /// 解析完成但装备或卖家没有通过核验。
    Ineligible {
        /// 可私有展示的安全装备信息。
        equipment: ListingEquipmentDetails,
        /// 私有拒绝原因。
        reason: ListingReason,
    },
    /// 非寄售公开展示。
    Displayed(ListingEquipmentDetails),
    /// 等待寄送装备，前三期禁止收件。
    AwaitingDeposit(ListingEquipmentDetails),
    /// 有效关闭，保留已取得的安全快照。
    Closed {
        /// 有效关闭原因。
        reason: ListingReason,
        /// 关闭前已取得的快照，可能没有。
        equipment: Option<ListingEquipmentDetails>,
    },
    /// 有效到期，保留已取得的安全快照。
    Expired {
        /// 有效到期原因。
        reason: ListingReason,
        /// 到期前已取得的快照，可能没有。
        equipment: Option<ListingEquipmentDetails>,
    },
}

impl PrivateListingViewKind {
    const fn effective_state(&self) -> (ListingState, Option<ListingReason>) {
        match self {
            Self::Resolving(_) => (ListingState::Resolving, None),
            Self::LookupFailed { reason, .. } | Self::Ineligible { reason, .. } => {
                (ListingState::Rejected, Some(*reason))
            }
            Self::Displayed(_) => (ListingState::Displayed, None),
            Self::AwaitingDeposit(_) => (ListingState::AwaitingDeposit, None),
            Self::Closed { reason, .. } => (ListingState::Closed, Some(*reason)),
            Self::Expired { reason, .. } => (ListingState::Expired, Some(*reason)),
        }
    }
}

/// 已核对模式、持久态、有效态和快照的一份私有视图。
#[derive(Debug, Clone, PartialEq)]
pub struct PrivateListingView {
    mode: ListingMode,
    stored_state: ListingState,
    kind: PrivateListingViewKind,
}

impl PrivateListingView {
    /// 构造同一读快照的投影。违反 §2.1、§4.1 或 §7.1 的持久形状视为依赖不可用。
    pub fn new(
        mode: ListingMode,
        stored_state: ListingState,
        kind: PrivateListingViewKind,
    ) -> Result<Self, ConsignmentError> {
        let (state, reason) = kind.effective_state();
        let expiry_source = match reason {
            Some(ListingReason::LookupDeadline) => Some(ListingState::Resolving),
            Some(ListingReason::DisplayExpired) => Some(ListingState::Displayed),
            Some(ListingReason::DepositExpired) => Some(ListingState::AwaitingDeposit),
            _ => None,
        };
        // 不变量 6：终态原因和模式必须相配，有效读只能提前反映强制终结。
        if reason.is_some_and(|reason| reason.terminal_state() != state.as_str())
            || !matches_mode(stored_state, mode)
            || !matches_mode(state, mode)
            || expiry_source.is_some_and(|source| !matches_mode(source, mode))
        {
            return Err(ConsignmentError::Unavailable);
        }
        if stored_state != state
            && !(stored_state.is_active()
                && (matches!(
                    reason,
                    Some(
                        ListingReason::AuctionLotCreated
                            | ListingReason::SellerIneligible
                            | ListingReason::ServiceDisabled
                    )
                ) || (state == ListingState::Expired && expiry_source == Some(stored_state))))
        {
            return Err(ConsignmentError::Unavailable);
        }
        match &kind {
            PrivateListingViewKind::LookupFailed { reason, .. }
                if rejected_with_snapshot(*reason) =>
            {
                return Err(ConsignmentError::Unavailable);
            }
            PrivateListingViewKind::Ineligible { reason, equipment }
                if !rejected_with_snapshot(*reason)
                    || equipment.label().tradeable == (*reason == ListingReason::NotTradeable) =>
            {
                return Err(ConsignmentError::Unavailable);
            }
            PrivateListingViewKind::Displayed(equipment)
            | PrivateListingViewKind::AwaitingDeposit(equipment)
                if !can_publish(equipment) =>
            {
                return Err(ConsignmentError::Unavailable);
            }
            PrivateListingViewKind::Closed { equipment, .. }
            | PrivateListingViewKind::Expired { equipment, .. } => {
                // 不变量 6：终结保留原有快照。已落库的关闭记录若有快照，只能来自已发布态，
                // 因为解析中没有快照，已拒绝的记录也不会转为关闭。
                let source = expiry_source.unwrap_or(stored_state);
                if (matches!(source, ListingState::Displayed | ListingState::AwaitingDeposit)
                    && equipment.is_none())
                    || (source == ListingState::Resolving && equipment.is_some())
                    || equipment.as_ref().is_some_and(|equipment| !can_publish(equipment))
                {
                    return Err(ConsignmentError::Unavailable);
                }
            }
            _ => {}
        }
        Ok(Self { mode, stored_state, kind })
    }

    /// 登记时确定的模式。
    pub const fn mode(&self) -> ListingMode {
        self.mode
    }

    /// 已提交的数据库状态。
    pub const fn stored_state(&self) -> ListingState {
        self.stored_state
    }

    /// 由同一有效视图导出有效态和有效原因，不伪造新版本。
    pub const fn effective_state(&self) -> (ListingState, Option<ListingReason>) {
        self.kind.effective_state()
    }

    /// 借用已通过一致性核验的视图内容。
    pub const fn kind(&self) -> &PrivateListingViewKind {
        &self.kind
    }
}

fn can_publish(equipment: &ListingEquipmentDetails) -> bool {
    let label = equipment.label();
    label.tradeable && label.level != ListingEquipmentLevel::Soulbound
}

fn matches_mode(state: ListingState, mode: ListingMode) -> bool {
    match state {
        ListingState::Displayed => mode == ListingMode::Direct,
        ListingState::AwaitingDeposit => mode == ListingMode::Consigned,
        _ => true,
    }
}

fn rejected_with_snapshot(reason: ListingReason) -> bool {
    matches!(
        reason,
        ListingReason::NotTradeable
            | ListingReason::OwnerMismatch
            | ListingReason::HistoryOwnerMismatch
            | ListingReason::HistoryAliasConflict
            | ListingReason::HistoryUnproven
            | ListingReason::EquipmentBusy
    )
}
