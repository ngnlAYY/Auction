//! 本人挂单的持久状态、有效状态和带判别键的展示投影。

use auction_time::Timestamp;
use uuid::Uuid;

use crate::deserialize_required_option;
use crate::dto::public::consignment::catalog::EquipmentView;
use crate::dto::public::consignment::scalar::ListingFeeBps;
use crate::dto::public::consignment::scalar::ListingPrice;
use crate::dto::public::consignment::scalar::ListingPricingVersion;
use crate::dto::public::consignment::scalar::ListingVersion;
use crate::dto::public::consignment::states::ListingAction;
use crate::dto::public::consignment::states::ListingMode;
use crate::dto::public::consignment::states::ListingReason;
use crate::dto::public::consignment::states::ListingState;
use crate::dto::public::consignment::states::ListingWorld;
use crate::dto::public::consignment::states::LookupReason;
use crate::dto::public::consignment::wire::equipment_id;
use crate::dto::public::consignment::wire::integer;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::optional_timestamp;
use crate::dto::public::consignment::wire::serialize_integer;
use crate::dto::public::consignment::wire::serialize_unavailable;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::unavailable;
use crate::dto::public::consignment::wire::uuid;

const MAX_LOOKUP_ATTEMPTS: i64 = 6;

json_object! {
    ()
    /// 仅含安全状态的解析进度，不携带工作租约或实例标识。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct LookupProgress {
        #[serde(
            deserialize_with = "integer::<0, MAX_LOOKUP_ATTEMPTS, _>",
            serialize_with = "serialize_integer::<0, MAX_LOOKUP_ATTEMPTS, _>"
        )]
        pub attempts: i64,
        #[serde(
            deserialize_with = "integer::<MAX_LOOKUP_ATTEMPTS, MAX_LOOKUP_ATTEMPTS, _>",
            serialize_with = "serialize_integer::<MAX_LOOKUP_ATTEMPTS, MAX_LOOKUP_ATTEMPTS, _>"
        )]
        pub max_attempts: i64,
        #[serde(deserialize_with = "optional_timestamp")]
        pub next_attempt_at: Option<Timestamp>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub last_reason: Option<LookupReason>,
        pub paused: bool,
        #[serde(deserialize_with = "timestamp")]
        pub deadline_at: Timestamp,
    }
}

json_object! {
    (tag = "kind", rename_all = "snake_case")
    /// 由有效状态决定的本人或运营视图，解析中及解析失败都可在没有快照时返回。
    #[derive(Debug, Clone, PartialEq)]
    pub enum ListingView {
        /// 尚在解析。
        Resolving { progress: LookupProgress },
        /// 未取得可信快照即终结。
        LookupFailed {
            reason: ListingReason,
            #[serde(deserialize_with = "deserialize_required_option")]
            last_lookup_reason: Option<LookupReason>,
        },
        /// 有安全快照，但未通过发布资格。
        Ineligible { equipment: EquipmentView, reason: ListingReason },
        /// 非寄售广告正在展示。
        Displayed { equipment: EquipmentView },
        /// 已通过核验，尚未代管。
        AwaitingDeposit {
            equipment: EquipmentView,
            #[serde(deserialize_with = "unavailable", serialize_with = "serialize_unavailable")]
            accepts_mail: bool,
        },
        /// 已关闭，包括尚未落库的强制关闭投影。
        Closed {
            reason: ListingReason,
            #[serde(deserialize_with = "deserialize_required_option")]
            equipment: Option<EquipmentView>,
        },
        /// 已到期，保留原有快照（如果存在）。
        Expired {
            reason: ListingReason,
            #[serde(deserialize_with = "deserialize_required_option")]
            equipment: Option<EquipmentView>,
        },
    } => validate
}

impl ListingView {
    fn state_and_reason(&self) -> (ListingState, Option<ListingReason>) {
        match self {
            Self::Resolving { .. } => (ListingState::Resolving, None),
            Self::Displayed { .. } => (ListingState::Displayed, None),
            Self::AwaitingDeposit { .. } => (ListingState::AwaitingDeposit, None),
            Self::Ineligible { reason, .. } | Self::LookupFailed { reason, .. } => {
                (ListingState::Rejected, Some(*reason))
            }
            Self::Closed { reason, .. } => (ListingState::Closed, Some(*reason)),
            Self::Expired { reason, .. } => (ListingState::Expired, Some(*reason)),
        }
    }

    fn validate(&self) -> Result<(), &'static str> {
        let (state, reason) = self.state_and_reason();
        if reason.is_some_and(|reason| reason.terminal_state() != state) {
            return Err("寄售原因与视图终态不一致");
        }
        match self {
            Self::LookupFailed { reason, .. } if reason.rejected_with_snapshot() => {
                Err("该拒挂原因必须使用带快照的视图")
            }
            Self::Ineligible { reason, equipment }
                if !reason.rejected_with_snapshot()
                    || equipment.tradeable == (*reason == ListingReason::NotTradeable) =>
            {
                Err("拒挂原因与装备快照不一致")
            }
            Self::Displayed { equipment } | Self::AwaitingDeposit { equipment, .. } => {
                equipment.validate_tradeable()
            }
            _ => Ok(()),
        }
    }
}

json_object! {
    ()
    /// 本人列表与详情共用投影。有效状态先于落库时，版本仍取已提交值。
    #[derive(Debug, Clone, PartialEq)]
    pub struct OwnedListing {
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        pub version: ListingVersion,
        pub world_id: ListingWorld,
        #[serde(deserialize_with = "equipment_id")]
        pub external_equipment_id: String,
        pub mode: ListingMode,
        pub price: ListingPrice,
        pub pricing_version: ListingPricingVersion,
        pub fee_bps: ListingFeeBps,
        pub estimated_fee: i64,
        pub estimated_seller_amount: i64,
        #[serde(deserialize_with = "timestamp")]
        pub created_at: Timestamp,
        #[serde(deserialize_with = "timestamp")]
        pub updated_at: Timestamp,
        #[serde(deserialize_with = "timestamp")]
        pub expires_at: Timestamp,
        pub stored_state: ListingState,
        pub effective_state: ListingState,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub effective_reason: Option<ListingReason>,
        pub allowed_actions: Vec<ListingAction>,
        pub view: ListingView,
    } => validate
}

impl OwnedListing {
    fn validate(&self) -> Result<(), &'static str> {
        // 不变量 6：有效态可以先看到强制终结，但不能复活或跳过已提交的发布。
        let (state, reason) = self.view.state_and_reason();
        let expiry_source = reason.and_then(ListingReason::expiry_source_state);
        if (self.effective_state, self.effective_reason) != (state, reason)
            || !self.stored_state.matches_mode(self.mode)
            || !state.matches_mode(self.mode)
            || expiry_source.is_some_and(|source| !source.matches_mode(self.mode))
        {
            return Err("寄售视图、状态、原因或模式不一致");
        }
        if self.stored_state != state
            && !(self.stored_state.is_active()
                && (reason.is_some_and(ListingReason::is_forced_closure)
                    || (state == ListingState::Expired
                        && expiry_source == Some(self.stored_state))))
        {
            return Err("寄售持久状态与有效状态不一致");
        }
        if !state.is_active() && !self.allowed_actions.is_empty() {
            return Err("已终结的挂单不得提供卖家操作");
        }
        if let ListingView::Closed { equipment, .. } | ListingView::Expired { equipment, .. } =
            &self.view
        {
            // 不变量 6：可确定前态时保留发布时的可交易快照；已落库的关闭原因无法区分前态。
            match (expiry_source.unwrap_or(self.stored_state), equipment) {
                (ListingState::Displayed | ListingState::AwaitingDeposit, Some(equipment)) => {
                    equipment.validate_tradeable()?;
                }
                (ListingState::Displayed | ListingState::AwaitingDeposit, None) => {
                    return Err("已发布的挂单终结时必须保留装备快照");
                }
                (ListingState::Resolving, Some(_)) => {
                    return Err("解析期终结的挂单不得携带装备快照");
                }
                _ => {}
            }
        }
        self.validate_quote()
    }

    fn validate_quote(&self) -> Result<(), &'static str> {
        let price = self.price.get();
        let fee_bps = self.fee_bps.get();
        if self.mode == ListingMode::Direct && (fee_bps != 0 || self.estimated_fee != 0) {
            return Err("非寄售挂单的费率与手续费必须为零");
        }
        // 不变量 6、10：领域 quote_v1 负责计算。这里只核对线上报价的取整和金额守恒。
        if self.estimated_fee != price * fee_bps / 10_000
            || self.estimated_seller_amount != price - self.estimated_fee
        {
            return Err("寄售预计手续费或卖家到手金额与冻结报价不一致");
        }
        Ok(())
    }
}
