//! v2 本人与 Admin 读模型端口：含代管态、处分代、止售与允许动作。
//!
//! 记录类型独立于 v1 的 `PrivateListingViewKind`，旧映射因此不必认识新状态；v1 读接口对这些行
//! 的隐藏由存储侧的可表达性过滤负责。允许动作只由领域的 `seller_actions`/`admin_actions`
//! 产生，本模块不再写第二份规则。

use auction_domain::UserId;
use auction_domain::consignment::deposit::{
    ActionFacts, AdminAction, PositionState, SellerAction, admin_actions, seller_actions,
};
use auction_domain::consignment::{
    FeeBps, HeldOffMarketReason, ListingEquipmentIdentity, ListingMode, ListingPrice, ListingState,
    ListingVersion,
};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::commands::ListingDelistReason;
use crate::ports::consignment::equipment::ListingEquipmentDetails;
use crate::ports::consignment::errors::{ConsignmentError, ConsignmentFuture};
use crate::ports::consignment::projection::PrivateListingView;
use crate::ports::consignment::reads::{
    AdminListingQuery, ListingAuctionConflict, ListingReadPage, PrivateListing, PrivateListingQuery,
};

/// 挂单的代管 position 摘要；不含原信键、装备 key 或证明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionSummary {
    /// position UUID。
    pub position_id: Uuid,
    /// 当前状态。
    pub state: PositionState,
    /// position 版本。
    pub version: u64,
    /// 当前处分代；尚未取得实物时为空。
    pub generation: Option<u64>,
}

/// 尚未解除的运营止售。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveHold {
    /// `hold_set` 控制行 UUID。
    pub hold_id: Uuid,
    /// 设止售时刻。
    pub set_at: Timestamp,
}

/// 同一读快照里的代管事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CustodyFacts {
    /// 挂单的 position；从未代管为空。
    pub position: Option<PositionSummary>,
    /// 未解除的运营止售。
    pub hold: Option<ActiveHold>,
    /// 存在撤回意愿控制行。
    pub withdrawal_intent_registered: bool,
    /// 该 position 的损失案件（J3）；没有案件为空。
    pub loss_case_id: Option<Uuid>,
    /// 此刻可发起手动重列：只有本人详情按重列写端同一判据填充，列表与 Admin 读恒为否。
    pub relist_allowed: bool,
}

/// v2 有效视图：v1 可表达的解析、展示、待寄、拒绝、到期与关闭，加上第 4 期的代管态。
#[derive(Debug, Clone, PartialEq)]
pub enum PrivateListingViewV2 {
    /// v1 已核验的视图。
    Legacy(PrivateListingView),
    /// 整封已接受，等待原 Take。
    DepositAccepted(ListingEquipmentDetails),
    /// 已代管并上架。
    Available(ListingEquipmentDetails),
    /// 已成立 Sale，正在履约。
    SalePending(ListingEquipmentDetails),
    /// 在库下架。
    HeldOffMarket {
        /// 安全装备快照。
        equipment: ListingEquipmentDetails,
        /// D9 §1.2 的在库下架原因。
        reason: HeldOffMarketReason,
    },
    /// 撤回意愿或 Withdrawal 正在履行。
    WithdrawalPending(ListingEquipmentDetails),
}

impl PrivateListingViewV2 {
    /// 有效状态。
    pub const fn effective_state(&self) -> ListingState {
        match self {
            Self::Legacy(view) => view.effective_state().0,
            Self::DepositAccepted(_) => ListingState::DepositAccepted,
            Self::Available(_) => ListingState::Available,
            Self::SalePending(_) => ListingState::SalePending,
            Self::HeldOffMarket { .. } => ListingState::HeldOffMarket,
            Self::WithdrawalPending(_) => ListingState::WithdrawalPending,
        }
    }

    /// 有效原因的稳定码；v1 终态原因与在库下架原因共用一个 wire 闭集。
    pub const fn effective_reason_code(&self) -> Option<&'static str> {
        match self {
            Self::Legacy(view) => match view.effective_state().1 {
                Some(reason) => Some(reason.as_str()),
                None => None,
            },
            Self::HeldOffMarket { reason, .. } => Some(reason.as_str()),
            _ => None,
        }
    }

    /// 持久状态；代管态的有效态即持久态（强制终结只作用于 v1 活动态与 available）。
    pub const fn stored_state(&self) -> ListingState {
        match self {
            Self::Legacy(view) => view.stored_state(),
            _ => self.effective_state(),
        }
    }

    /// 登记模式；代管态只会来自寄售。
    pub const fn mode(&self) -> ListingMode {
        match self {
            Self::Legacy(view) => view.mode(),
            _ => ListingMode::Consigned,
        }
    }
}

/// 本人 / Admin 共用的 v2 挂单事实。
#[derive(Debug, Clone, PartialEq)]
pub struct PrivateListingV2 {
    /// 挂单身份。
    pub listing_id: Uuid,
    /// 持久版本。
    pub version: ListingVersion,
    /// 规范世界和 EID。
    pub identity: ListingEquipmentIdentity,
    /// 当前标价。
    pub price: ListingPrice,
    /// 冻结定价算法版本。
    pub pricing_version: u16,
    /// 登记时冻结的费率。
    pub fee_bps: FeeBps,
    /// 创建时刻。
    pub created_at: Timestamp,
    /// 最近持久变化。
    pub updated_at: Timestamp,
    /// 当前态冻结截止。
    pub expires_at: Timestamp,
    /// 有效视图。
    pub view: PrivateListingViewV2,
    /// 卖家经站内账户登记；匿名非寄售挂单为否，只能由同名账户撤回。
    pub seller_verified: bool,
    /// 代管事实。
    pub custody: CustodyFacts,
}

impl PrivateListingV2 {
    /// 把 v1 已核验的挂单与代管事实组合成 v2 记录；v1 视图只能带历史 position，不会带活动止售。
    pub fn from_legacy(
        listing: PrivateListing,
        seller_verified: bool,
        custody: CustodyFacts,
    ) -> Result<Self, ConsignmentError> {
        let PrivateListing {
            listing_id,
            version,
            identity,
            price,
            pricing_version,
            fee_bps,
            created_at,
            updated_at,
            expires_at,
            view,
        } = listing;
        Self {
            listing_id,
            version,
            identity,
            price,
            pricing_version,
            fee_bps,
            created_at,
            updated_at,
            expires_at,
            view: PrivateListingViewV2::Legacy(view),
            seller_verified,
            custody,
        }
        .validated()
    }

    /// 不变量 6：违反 §1.2 形状的持久行视为依赖不可用，不静默修正。
    pub fn validated(self) -> Result<Self, ConsignmentError> {
        let custody = &self.custody;
        // 损失案件按 position 建立：有案件却没有 position 的行违反持久形状。
        if custody.loss_case_id.is_some() && custody.position.is_none() {
            return Err(ConsignmentError::Unavailable);
        }
        let generation = custody.position.and_then(|position| position.generation);
        // 匿名登记只有非寄售，不可能进入代管视图。
        let valid = match &self.view {
            PrivateListingViewV2::Legacy(_) => true,
            _ if !self.seller_verified => false,
            // 代管态必有 position；取得后才有处分代，已接受未取得的件只能没有。
            PrivateListingViewV2::DepositAccepted(_) => custody.position.is_some(),
            PrivateListingViewV2::Available(_)
            | PrivateListingViewV2::SalePending(_)
            | PrivateListingViewV2::WithdrawalPending(_) => generation.is_some(),
            // 只有 position 处于 awaiting_take 且原因为运营止售（已接受未取得，对其设止售）时
            // 可没有处分代，与本人 DTO 同口径。
            PrivateListingViewV2::HeldOffMarket { reason, .. } => {
                (generation.is_some()
                    || (*reason == HeldOffMarketReason::OperationsHold
                        && custody
                            .position
                            .is_some_and(|position| position.state == PositionState::AwaitingTake)))
                    && (*reason != HeldOffMarketReason::OperationsHold || custody.hold.is_some())
            }
        };
        if !valid {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(self)
    }

    /// 处分代；没有代管 position 或尚未取得时为空。
    pub fn generation(&self) -> Option<u64> {
        self.custody.position.and_then(|position| position.generation)
    }

    /// 领域动作事实。
    fn action_facts(&self) -> ActionFacts {
        ActionFacts {
            effective_state: self.view.effective_state(),
            held_reason: match &self.view {
                PrivateListingViewV2::HeldOffMarket { reason, .. } => Some(*reason),
                _ => None,
            },
            active_hold: self.custody.hold.is_some(),
            withdrawal_intent_registered: self.custody.withdrawal_intent_registered,
            position: self.custody.position.map(|position| position.state),
            // 重列许可与损失案件都按读快照填充（J3）；重列最终仍由写端锁内裁决。
            relist_allowed: self.custody.relist_allowed,
            loss_case_id: self.custody.loss_case_id,
            seller_verified: self.seller_verified,
        }
    }

    /// 本人此刻可执行的动作，唯一来源是领域 `seller_actions`。
    pub fn seller_actions(&self) -> Vec<SellerAction> {
        seller_actions(&self.action_facts())
    }

    /// Admin 此刻可执行的动作，唯一来源是领域 `admin_actions`。
    pub fn admin_actions(&self) -> Vec<AdminAction> {
        admin_actions(&self.action_facts())
    }
}

/// Admin 列表行。
#[derive(Debug, Clone, PartialEq)]
pub struct AdminListingRecordV2 {
    /// 管理员可读的挂单事实。
    pub listing: PrivateListingV2,
    /// 真正的本地卖家；匿名非寄售挂单没有本站账户。
    pub seller_user_id: Option<UserId>,
    /// 当前本站卖家名。
    pub seller_name: String,
    /// 仅运营会话可查看的下架说明。
    pub admin_delist_reason: Option<ListingDelistReason>,
}

/// Admin 详情，含最近 50 条碰撞。
#[derive(Debug, Clone, PartialEq)]
pub struct AdminListingDetailV2 {
    /// 管理员读投影。
    pub record: AdminListingRecordV2,
    /// 按 `detected_at DESC,lot_id DESC` 排序。
    pub conflicts: Vec<ListingAuctionConflict>,
    /// 尚有更早的关联。
    pub conflicts_truncated: bool,
}

/// 仅从已守卫的会话取得卖家 ID，再读取本人 v2 历史。
pub trait OwnedListingReadV2: Send + Sync {
    /// 按卖家和有效态筛选，同快照取总数。
    fn list_owned_v2(
        &self,
        seller_user_id: UserId,
        query: PrivateListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<PrivateListingV2>>;

    /// 非本人和不存在均返回 `None`。
    fn owned_detail_v2(
        &self,
        seller_user_id: UserId,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<PrivateListingV2>>;
}

/// Admin 授权后的 v2 只读端口。
pub trait AdminListingReadV2: Send + Sync {
    /// 根据有效态及可选卖家筛选，同快照取总数。
    fn list_admin_v2(
        &self,
        query: AdminListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<AdminListingRecordV2>>;

    /// 查询私有详情和安全碰撞关联。
    fn admin_detail_v2(
        &self,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<AdminListingDetailV2>>;
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::deposit::PositionState;
    use auction_domain::consignment::{
        FeeBps, HeldOffMarketReason, ListingEquipmentIdentity, ListingPrice, ListingVersion,
    };
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{
        ActiveHold, CustodyFacts, PositionSummary, PrivateListingV2, PrivateListingViewV2,
    };
    use crate::ports::consignment::equipment::{
        ListingEquipmentDetails, ListingEquipmentLabel, ListingEquipmentLevel,
    };

    fn time() -> Timestamp {
        Timestamp::parse_from_rfc3339("2026-11-21T00:00:00Z").expect("夹具时刻")
    }

    fn held(
        reason: HeldOffMarketReason,
        position: PositionState,
        generation: Option<u64>,
    ) -> Result<PrivateListingV2, crate::ports::consignment::errors::ConsignmentError> {
        let equipment = ListingEquipmentDetails::new(
            time(),
            ListingEquipmentLabel {
                name: "Fixture Oak Staff".to_owned(),
                alt_name: None,
                category: "Staff".to_owned(),
                level: ListingEquipmentLevel::Assigned(42),
                tradeable: true,
            },
            Vec::new(),
        )
        .expect("夹具装备");
        let hold = (reason == HeldOffMarketReason::OperationsHold)
            .then(|| ActiveHold { hold_id: Uuid::from_u128(9), set_at: time() });
        PrivateListingV2 {
            listing_id: Uuid::from_u128(1),
            version: ListingVersion::new(5).expect("版本"),
            identity: ListingEquipmentIdentity::new("main", "42").expect("装备身份"),
            price: ListingPrice::new(101).expect("标价"),
            pricing_version: 1,
            fee_bps: FeeBps::DEFAULT_CONSIGNED,
            created_at: time(),
            updated_at: time(),
            expires_at: time(),
            view: PrivateListingViewV2::HeldOffMarket { equipment, reason },
            seller_verified: true,
            custody: CustodyFacts {
                position: Some(PositionSummary {
                    position_id: Uuid::from_u128(2),
                    state: position,
                    version: 2,
                    generation,
                }),
                hold,
                withdrawal_intent_registered: false,
                loss_case_id: None,
                relist_allowed: false,
            },
        }
        .validated()
    }

    /// 不变量 6：只有 position 处于 awaiting_take 的在库下架件可以没有处分代，其余失败关闭。
    #[test]
    fn held_off_market_without_generation_requires_an_awaiting_take_position() {
        let operations = HeldOffMarketReason::OperationsHold;
        assert!(held(operations, PositionState::AwaitingTake, None).is_ok());
        assert!(held(operations, PositionState::InCustody, None).is_err());
        assert!(held(operations, PositionState::InCustody, Some(1)).is_ok());
        assert!(
            held(HeldOffMarketReason::QuotaBlocked, PositionState::AwaitingTake, None).is_err()
        );
        assert!(held(HeldOffMarketReason::QuotaBlocked, PositionState::InCustody, None).is_err());
    }
}
