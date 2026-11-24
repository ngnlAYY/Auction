//! v2 挂单状态、视图与投影的严格校验。

use super::ListingActionV2;
use super::ListingReasonV2;
use super::ListingStateV2;
use super::ListingSummaryV2;
use super::ListingViewV2;
use super::OwnedListingV2;
use super::PublicListingStateV2;
use crate::EquipmentLevel;
use crate::dto::public::consignment::states::ListingMode;

impl ListingStateV2 {
    /// 是否仍可沿状态机继续，而不是 rejected/expired/closed 终态。
    pub const fn is_live(self) -> bool {
        !matches!(self, Self::Rejected | Self::Expired | Self::Closed)
    }

    pub(super) const fn matches_mode(self, mode: ListingMode) -> bool {
        match self {
            Self::Displayed => matches!(mode, ListingMode::Direct),
            Self::AwaitingDeposit
            | Self::DepositAccepted
            | Self::Available
            | Self::SalePending
            | Self::HeldOffMarket
            | Self::WithdrawalPending => matches!(mode, ListingMode::Consigned),
            _ => true,
        }
    }
}

impl ListingReasonV2 {
    /// D9 §1.2 的 `held_off_market` 原因闭集。
    pub const fn is_hold_reason(self) -> bool {
        matches!(
            self,
            Self::UnpaidRecovered
                | Self::PaidReturned
                | Self::SellerIneligible
                | Self::ServiceDisabled
                | Self::QuotaBlocked
                | Self::OperationsHold
                | Self::Review
        )
    }

    /// 关闭原因闭集，含第 4 期新增的四种关闭原因。
    pub const fn is_closed_reason(self) -> bool {
        matches!(
            self,
            Self::AuctionLotCreated
                | Self::SellerIneligible
                | Self::ServiceDisabled
                | Self::SellerCancelled
                | Self::AdminDelisted
                | Self::Sold
                | Self::WithdrawnDelivered
                | Self::SeasonClosed
                | Self::LossConfirmed
        )
    }

    /// 到期原因闭集。
    pub const fn is_expiry_reason(self) -> bool {
        matches!(self, Self::LookupDeadline | Self::DisplayExpired | Self::DepositExpired)
    }

    /// 未取得可信快照即终结的原因闭集。
    pub const fn is_lookup_failure(self) -> bool {
        matches!(
            self,
            Self::OwnerMissing
                | Self::EquipmentNotFound
                | Self::SourceRequestRejected
                | Self::SourceHttpInvalid
                | Self::SourceSchemaInvalid
                | Self::SourceShapeUnconfirmed
                | Self::SourceIdentityMismatch
                | Self::SourceTooLarge
                | Self::LookupExhausted
                | Self::CredentialUnavailable
        )
    }

    /// 已取得安全快照后产生的拒挂原因。
    pub const fn rejected_with_snapshot(self) -> bool {
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

    /// 读侧可先于落库观察到的强制关闭原因。
    pub const fn is_forced_closure(self) -> bool {
        matches!(self, Self::AuctionLotCreated | Self::SellerIneligible | Self::ServiceDisabled)
    }

    /// 到期原因对应的前态。
    pub const fn expiry_source_state(self) -> Option<ListingStateV2> {
        match self {
            Self::LookupDeadline => Some(ListingStateV2::Resolving),
            Self::DisplayExpired => Some(ListingStateV2::Displayed),
            Self::DepositExpired => Some(ListingStateV2::AwaitingDeposit),
            _ => None,
        }
    }

    /// 原因所属的终态或在库下架态；活动原因及同时属于 held/closed 的原因返回 None。
    pub const fn terminal_state(self) -> Option<ListingStateV2> {
        // seller_ineligible/service_disabled 同时属于 held 与 closed 闭集，
        // 必须由视图上下文决定，不能在这里归一成单一终态。
        if matches!(self, Self::SellerIneligible | Self::ServiceDisabled) {
            return None;
        }
        if self.is_hold_reason()
            && !self.is_closed_reason()
            && !self.is_expiry_reason()
            && !self.rejected_with_snapshot()
            && !self.is_lookup_failure()
        {
            return Some(ListingStateV2::HeldOffMarket);
        }
        if self.is_closed_reason() {
            return Some(ListingStateV2::Closed);
        }
        if self.is_expiry_reason() {
            return Some(ListingStateV2::Expired);
        }
        if self.rejected_with_snapshot() || self.is_lookup_failure() {
            return Some(ListingStateV2::Rejected);
        }
        None
    }
}

impl ListingActionV2 {
    pub(super) const fn allowed_in(
        self,
        state: ListingStateV2,
        reason: Option<ListingReasonV2>,
    ) -> bool {
        match self {
            Self::Reprice => matches!(
                state,
                ListingStateV2::Resolving
                    | ListingStateV2::Displayed
                    | ListingStateV2::AwaitingDeposit
                    | ListingStateV2::Available
            ),
            Self::Withdraw => matches!(
                state,
                ListingStateV2::Resolving
                    | ListingStateV2::Displayed
                    | ListingStateV2::AwaitingDeposit
                    | ListingStateV2::DepositAccepted
                    | ListingStateV2::Available
                    | ListingStateV2::HeldOffMarket
            ),
            // 决定 24：运营止售的件只能撤回，不能手动或自动重新上架。
            Self::Relist => match (state, reason) {
                (ListingStateV2::HeldOffMarket, Some(reason)) => {
                    !matches!(reason, ListingReasonV2::OperationsHold)
                }
                _ => false,
            },
            Self::IssueReservation => matches!(state, ListingStateV2::AwaitingDeposit),
            Self::Purchase | Self::AdminDelist | Self::ReleaseHold => false,
        }
    }
}
impl ListingSummaryV2 {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if self.listing_id.is_nil() {
            return Err("挂单标识不得为零 UUID");
        }
        let mode_matches = match self.state {
            PublicListingStateV2::Displayed => matches!(self.mode, ListingMode::Direct),
            PublicListingStateV2::AwaitingDeposit | PublicListingStateV2::Available => {
                matches!(self.mode, ListingMode::Consigned)
            }
        };
        if !mode_matches {
            return Err("公开挂单的模式与状态不一致");
        }
        if self.effective_reason.is_some() {
            return Err("公开挂单不得携带私有原因");
        }
        if matches!(self.level, EquipmentLevel::Soulbound {}) {
            return Err("公开挂单不得展示灵魂绑定装备");
        }
        match self.state {
            PublicListingStateV2::Displayed | PublicListingStateV2::AwaitingDeposit => {
                if self.generation.is_some() {
                    return Err("未代管的公开挂单不得携带处分代");
                }
            }
            PublicListingStateV2::Available => {
                if self.generation.is_none() {
                    return Err("已上架的公开挂单必须携带处分代");
                }
            }
        }
        if has_duplicates(&self.allowed_actions) {
            return Err("公开挂单的允许动作不得重复");
        }
        let purchase =
            self.allowed_actions.iter().all(|action| *action == ListingActionV2::Purchase);
        if !purchase {
            return Err("公开挂单只能宣告购买动作");
        }
        let can_purchase = self.allowed_actions.len() == 1
            && self.allowed_actions.first() == Some(&ListingActionV2::Purchase);
        if self.purchasable != can_purchase {
            return Err("公开挂单的购买能力与允许动作不一致");
        }
        if self.purchasable && self.state != PublicListingStateV2::Available {
            return Err("只有 available 公开挂单可以购买");
        }
        Ok(())
    }
}
impl OwnedListingV2 {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if self.listing_id.is_nil() {
            return Err("挂单标识不得为零 UUID");
        }
        let (state, reason) = self.view.state_and_reason();
        let expiry_source = reason.and_then(ListingReasonV2::expiry_source_state);
        if (self.effective_state, self.effective_reason) != (state, reason)
            || !self.stored_state.matches_mode(self.mode)
            || !state.matches_mode(self.mode)
            || expiry_source.is_some_and(|source| !source.matches_mode(self.mode))
        {
            return Err("寄售视图、状态、原因或模式不一致");
        }
        // 不变量 6：有效态可以先看到强制终结或本态到期，但不能复活或跳过已提交的发布。
        if self.stored_state != state
            && !(self.stored_state.is_live()
                && (reason.is_some_and(ListingReasonV2::is_forced_closure)
                    || (state == ListingStateV2::Expired
                        && expiry_source == Some(self.stored_state))))
        {
            return Err("寄售持久状态与有效状态不一致");
        }
        if !state.is_live() && !self.allowed_actions.is_empty() {
            return Err("已终结的挂单不得提供卖家操作");
        }
        if has_duplicates(&self.allowed_actions) {
            return Err("挂单允许动作不得重复");
        }
        if self.allowed_actions.iter().any(|action| !action.allowed_in(state, reason)) {
            return Err("挂单允许动作与有效状态不一致");
        }
        self.validate_generation(state)?;
        self.validate_snapshot(expiry_source, state)?;
        self.validate_quote()
    }

    fn validate_generation(&self, state: ListingStateV2) -> Result<(), &'static str> {
        let expected = match state {
            ListingStateV2::Resolving
            | ListingStateV2::Displayed
            | ListingStateV2::AwaitingDeposit => Some(false),
            ListingStateV2::Available
            | ListingStateV2::SalePending
            | ListingStateV2::HeldOffMarket
            | ListingStateV2::WithdrawalPending => Some(true),
            ListingStateV2::DepositAccepted
            | ListingStateV2::Rejected
            | ListingStateV2::Expired
            | ListingStateV2::Closed => None,
        };
        if expected.is_some_and(|expected| expected != self.generation.is_some()) {
            return Err("处分代与挂单有效状态不一致");
        }
        Ok(())
    }

    fn validate_snapshot(
        &self,
        expiry_source: Option<ListingStateV2>,
        state: ListingStateV2,
    ) -> Result<(), &'static str> {
        let (reason, equipment) = match &self.view {
            ListingViewV2::Closed { reason, equipment } => (Some(*reason), equipment),
            ListingViewV2::Expired { reason, equipment } => (Some(*reason), equipment),
            _ => return Ok(()),
        };
        if let Some(equipment) = equipment {
            equipment.validate_tradeable()?;
        }
        let previous =
            expiry_source
                .or_else(|| {
                    if state == ListingStateV2::Closed { Some(self.stored_state) } else { None }
                })
                .unwrap_or(self.stored_state);
        match (previous, equipment) {
            (
                ListingStateV2::Displayed
                | ListingStateV2::AwaitingDeposit
                | ListingStateV2::DepositAccepted
                | ListingStateV2::Available
                | ListingStateV2::SalePending
                | ListingStateV2::HeldOffMarket
                | ListingStateV2::WithdrawalPending,
                None,
            ) => return Err("已发布的挂单终结时必须保留装备快照"),
            (ListingStateV2::Resolving, Some(_)) => {
                return Err("解析期终结的挂单不得携带装备快照");
            }
            _ => {}
        }
        if state == ListingStateV2::Closed && !reason.is_some_and(ListingReasonV2::is_closed_reason)
        {
            return Err("关闭视图的原因不属于关闭闭集");
        }
        if state == ListingStateV2::Expired
            && reason.and_then(ListingReasonV2::terminal_state) != Some(ListingStateV2::Expired)
        {
            return Err("到期原因与视图终态不一致");
        }
        Ok(())
    }

    fn validate_quote(&self) -> Result<(), &'static str> {
        let price = self.price.get();
        let fee_bps = self.fee_bps.get();
        if matches!(self.mode, ListingMode::Direct) && (fee_bps != 0 || self.estimated_fee != 0) {
            return Err("非寄售挂单的费率与手续费必须为零");
        }
        // 不变量 6、10：领域 quote 负责计算。这里只核对线上报价的取整和金额守恒。
        if self.estimated_fee != price * fee_bps / 10_000
            || self.estimated_seller_amount != price - self.estimated_fee
        {
            return Err("寄售预计手续费或卖家到手金额与冻结报价不一致");
        }
        Ok(())
    }
}
fn has_duplicates<T: PartialEq>(items: &[T]) -> bool {
    items.iter().enumerate().any(|(index, item)| items[..index].contains(item))
}
