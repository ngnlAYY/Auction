//! Admin v2 挂单读模型：本人视图之外的代管态、有效止售、position 与 Admin 允许动作。

use std::fmt;

use auction_time::Timestamp;
use uuid::Uuid;

use super::ListingAuctionConflict;
use super::optional_delist_reason;
use super::validate_delist_reason;
use crate::ListingGeneration;
use crate::ListingPage;
use crate::ListingStateV2;
use crate::ListingVersion;
use crate::OwnedListingV2;
use crate::deserialize_required_option;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::string_enum;

const MAX_AUCTION_CONFLICTS: usize = 50;

string_enum! {
    /// 代管 position 的独立状态闭集。
    pub enum PositionStateV2 {
        /// 已接受，等待精确 Take。
        AwaitingTake => "awaiting_take",
        /// 已取得实物，处于 Bot 代管。
        InCustody => "in_custody",
        /// 已由合同承诺给买家。
        Committed => "committed",
        /// 派发在途。
        InTransit => "in_transit",
        /// 证明冲突或结果未知，等待核对。
        Reconcile => "reconcile",
        /// 已交接给合法领取人。
        HandedOver => "handed_over",
        /// 旧季结束。
        SeasonEnded => "season_ended",
        /// 已确认损失。
        Lost => "lost",
    }
}

string_enum! {
    /// 服务器允许 Admin 对挂单执行的动作。
    pub enum AdminListingActionV2 {
        /// 下架：未代管件关闭，代管件设运营止售。
        Delist => "delist",
        /// 解除当前有效的运营止售。
        ReleaseHold => "release_hold",
    }
}

json_object! {
    ()
    /// 尚未解除的运营止售。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AdminHoldV2 {
        #[serde(deserialize_with = "uuid")]
        pub hold_id: Uuid,
        #[serde(deserialize_with = "timestamp")]
        pub set_at: Timestamp,
    } => validate
}

impl AdminHoldV2 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.hold_id.is_nil() {
            return Err("hold_id 不得为零 UUID");
        }
        Ok(())
    }
}

json_object! {
    ()
    /// 挂单的代管 position 摘要，不含原信键、装备 key 或证明。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AdminPositionV2 {
        #[serde(deserialize_with = "uuid")]
        pub position_id: Uuid,
        pub state: PositionStateV2,
        pub version: ListingVersion,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub generation: Option<ListingGeneration>,
    } => validate
}

impl AdminPositionV2 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.position_id.is_nil() {
            return Err("position_id 不得为零 UUID");
        }
        Ok(())
    }
}

json_object! {
    ()
    /// Admin v2 列表记录。本人视图的 `allowed_actions` 恒空，运营动作在 `admin_actions`。
    /// 私有说明沿用下架请求的凭据筛查，调试输出统一隐去说明。
    #[derive(Clone, PartialEq)]
    pub struct AdminListingV2 {
        pub listing: OwnedListingV2,
        #[serde(deserialize_with = "uuid")]
        pub seller_user_id: Uuid,
        pub seller_name: String,
        #[serde(deserialize_with = "optional_delist_reason")]
        pub admin_delist_reason: Option<String>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub hold: Option<AdminHoldV2>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub position: Option<AdminPositionV2>,
        pub admin_actions: Vec<AdminListingActionV2>,
    } => validate
}

impl AdminListingV2 {
    fn validate(&self) -> Result<(), &'static str> {
        if !self.listing.allowed_actions.is_empty() {
            return Err("Admin 挂单投影不得携带卖家操作");
        }
        if let Some(reason) = &self.admin_delist_reason {
            validate_delist_reason(reason)?;
        }
        let state = self.listing.effective_state;
        // hold_id 只在 Admin 的 `hold` 展示，本人视图不带；止售只在在库下架态展示。
        if self.hold.is_some() && state != ListingStateV2::HeldOffMarket {
            return Err("Admin 挂单的有效止售只能出现在在库下架态");
        }
        // 只有 position 处于 awaiting_take（已接受未取得）的在库下架件可以没有处分代。
        if state == ListingStateV2::HeldOffMarket
            && self.listing.generation.is_none()
            && self.position.map(|position| position.state) != Some(PositionStateV2::AwaitingTake)
        {
            return Err("在库下架件只有在 position 等待取得时才可没有处分代");
        }
        if self.position.is_some_and(|position| position.generation != self.listing.generation) {
            return Err("Admin 挂单的 position 处分代与挂单不一致");
        }
        self.validate_actions(state)
    }

    fn validate_actions(&self, state: ListingStateV2) -> Result<(), &'static str> {
        let delist = self.admin_actions.contains(&AdminListingActionV2::Delist);
        let release = self.admin_actions.contains(&AdminListingActionV2::ReleaseHold);
        if self.admin_actions.len() != usize::from(delist) + usize::from(release) {
            return Err("Admin 允许动作不得重复");
        }
        if delist && release {
            return Err("下架与解除止售不得同时允许");
        }
        if delist
            && (self.hold.is_some()
                || !matches!(
                    state,
                    ListingStateV2::Resolving
                        | ListingStateV2::Displayed
                        | ListingStateV2::AwaitingDeposit
                        | ListingStateV2::DepositAccepted
                        | ListingStateV2::Available
                        | ListingStateV2::HeldOffMarket
                ))
        {
            return Err("Admin 下架动作与有效状态或有效止售不一致");
        }
        if release && (self.hold.is_none() || state != ListingStateV2::HeldOffMarket) {
            return Err("解除止售动作必须有在库下架态的有效止售");
        }
        Ok(())
    }
}

impl fmt::Debug for AdminListingV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminListingV2")
            .field("listing", &self.listing)
            .field("seller_user_id", &self.seller_user_id)
            .field("seller_name", &self.seller_name)
            .field("admin_delist_reason", &self.admin_delist_reason.as_ref().map(|_| "[REDACTED]"))
            .field("hold", &self.hold)
            .field("position", &self.position)
            .field("admin_actions", &self.admin_actions)
            .finish()
    }
}

json_object! {
    ()
    /// Admin v2 详情及至多 50 条最近碰撞。
    #[derive(Debug, Clone, PartialEq)]
    pub struct AdminListingDetailV2 {
        pub record: AdminListingV2,
        pub conflicts: Vec<ListingAuctionConflict>,
        pub conflicts_truncated: bool,
    } => validate
}

impl AdminListingDetailV2 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.conflicts.len() > MAX_AUCTION_CONFLICTS {
            return Err("寄售详情的碰撞记录超出上限");
        }
        if self.conflicts_truncated && self.conflicts.len() != MAX_AUCTION_CONFLICTS {
            return Err("标记截断时必须返回达到上限的碰撞记录");
        }
        Ok(())
    }
}

/// 本人 v2 列表分页体。
pub type OwnedListingPageV2 = ListingPage<OwnedListingV2>;

/// Admin v2 列表分页体。
pub type AdminListingPageV2 = ListingPage<AdminListingV2>;
