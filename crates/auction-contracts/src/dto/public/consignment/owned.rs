//! 本人挂单的持久状态、有效状态和带判别键的展示投影。

use auction_time::Timestamp;
use serde::de::Error;
use uuid::Uuid;

use crate::deserialize_required_option;
use crate::dto::public::consignment::catalog::EquipmentView;
use crate::dto::public::consignment::states::ListingAction;
use crate::dto::public::consignment::states::ListingMode;
use crate::dto::public::consignment::states::ListingReason;
use crate::dto::public::consignment::states::ListingState;
use crate::dto::public::consignment::states::ListingWorld;
use crate::dto::public::consignment::states::LookupReason;
use crate::dto::public::consignment::wire::FEE_BPS_MAX;
use crate::dto::public::consignment::wire::PRICE_MAX;
use crate::dto::public::consignment::wire::SAFE_INTEGER_MAX;
use crate::dto::public::consignment::wire::equipment_id;
use crate::dto::public::consignment::wire::integer;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::optional_timestamp;
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
        #[serde(deserialize_with = "integer::<0, MAX_LOOKUP_ATTEMPTS, _>")]
        pub attempts: i64,
        #[serde(deserialize_with = "integer::<MAX_LOOKUP_ATTEMPTS, MAX_LOOKUP_ATTEMPTS, _>")]
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
    }
}

json_object! {
    ()
    /// 本人列表与详情共用投影。有效状态先于落库时，版本仍取已提交值。
    #[derive(Debug, Clone, PartialEq)]
    pub struct OwnedListing {
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        #[serde(deserialize_with = "integer::<1, SAFE_INTEGER_MAX, _>")]
        pub version: i64,
        pub world_id: ListingWorld,
        #[serde(deserialize_with = "equipment_id")]
        pub external_equipment_id: String,
        pub mode: ListingMode,
        #[serde(deserialize_with = "integer::<1, PRICE_MAX, _>")]
        pub price: i64,
        #[serde(deserialize_with = "integer::<1, 1, _>")]
        pub pricing_version: i64,
        #[serde(deserialize_with = "integer::<0, FEE_BPS_MAX, _>")]
        pub fee_bps: i64,
        #[serde(deserialize_with = "integer::<0, PRICE_MAX, _>")]
        pub estimated_fee: i64,
        #[serde(deserialize_with = "integer::<1, PRICE_MAX, _>")]
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
    fn validate<E: Error>(&self) -> Result<(), E> {
        // 不变量 6：先核对有效态投影，再让客户端使用允许动作；持久态可以比有效态滞后。
        let (state, reason) = match &self.view {
            ListingView::Resolving { .. } => (ListingState::Resolving, None),
            ListingView::Displayed { .. } => (ListingState::Displayed, None),
            ListingView::AwaitingDeposit { .. } => (ListingState::AwaitingDeposit, None),
            ListingView::Ineligible { reason, .. } | ListingView::LookupFailed { reason, .. } => {
                (ListingState::Rejected, Some(*reason))
            }
            ListingView::Closed { reason, .. } => (ListingState::Closed, Some(*reason)),
            ListingView::Expired { reason, .. } => (ListingState::Expired, Some(*reason)),
        };
        if (self.effective_state, self.effective_reason) != (state, reason) {
            return Err(E::custom("寄售视图与有效状态或原因不一致"));
        }
        if reason.is_some() && !self.allowed_actions.is_empty() {
            return Err(E::custom("已终结的挂单不得提供卖家操作"));
        }
        Ok(())
    }
}
