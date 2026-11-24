//! v2 公开与本人挂单读模型，覆盖第 4 期新增状态、处分代、原因和允许动作。

mod validation;

use auction_time::Timestamp;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use crate::deserialize_required_option;
use crate::dto::public::consignment::catalog::EquipmentLevel;
use crate::dto::public::consignment::catalog::EquipmentView;
use crate::dto::public::consignment::owned::LookupProgress;
use crate::dto::public::consignment::receipt::ConsignmentOperationV2;
use crate::dto::public::consignment::scalar::ListingFeeBps;
use crate::dto::public::consignment::scalar::ListingPrice;
use crate::dto::public::consignment::scalar::ListingPricingVersion;
use crate::dto::public::consignment::scalar::ListingVersion;
use crate::dto::public::consignment::states::ListingMode;
use crate::dto::public::consignment::states::ListingWorld;
use crate::dto::public::consignment::states::LookupReason;
use crate::dto::public::consignment::wire::equipment_id;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::text;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::command_uuid;
use crate::dto::public::consignment::wire_v2::positive_version;
use crate::dto::public::consignment::wire_v2::required_option_uuid;
use crate::dto::public::consignment::wire_v2::serialize_positive_version;
use crate::dto::public::consignment::wire_v2::string_enum;
use crate::dto::public::consignment::write::write_object;

string_enum! {
    /// 挂单持久态与有效态使用的完整 v2 闭集。
    pub enum ListingStateV2 {
        /// 解析中。
        Resolving => "resolving",
        /// 非寄售广告已展示。
        Displayed => "displayed",
        /// 寄售件等待寄来。
        AwaitingDeposit => "awaiting_deposit",
        /// 整封已接受，等待原 Take。
        DepositAccepted => "deposit_accepted",
        /// 唯一可购买态。
        Available => "available",
        /// 已成立 Sale，正在履约。
        SalePending => "sale_pending",
        /// 在库下架，原因见 `held_off_market` 闭集。
        HeldOffMarket => "held_off_market",
        /// 原卖家撤回意愿已登记或已建 Withdrawal。
        WithdrawalPending => "withdrawal_pending",
        /// 已拒绝。
        Rejected => "rejected",
        /// 已到期。
        Expired => "expired",
        /// 已关闭。
        Closed => "closed",
    }
}

string_enum! {
    /// 公开目录可表达的 v2 状态；私有状态在此类型中不可表达。
    pub enum PublicListingStateV2 {
        /// 非寄售广告已展示。
        Displayed => "displayed",
        /// 寄售件尚未代管，可公开标注。
        AwaitingDeposit => "awaiting_deposit",
        /// 已代管并上架。
        Available => "available",
    }
}

string_enum! {
    /// v2 挂单终态、在库下架与既有解析/拒挂原因。
    pub enum ListingReasonV2 {
        /// 装备已进入拍卖。
        AuctionLotCreated => "auction_lot_created",
        /// 卖家资格已失效。
        SellerIneligible => "seller_ineligible",
        /// 服务已停用。
        ServiceDisabled => "service_disabled",
        /// 卖家已取消。
        SellerCancelled => "seller_cancelled",
        /// 运营已下架。
        AdminDelisted => "admin_delisted",
        /// 解析期限已到。
        LookupDeadline => "lookup_deadline",
        /// 展示期限已到。
        DisplayExpired => "display_expired",
        /// 待寄期限已到。
        DepositExpired => "deposit_expired",
        /// 装备不可交易。
        NotTradeable => "not_tradeable",
        /// 来源缺少有效持有人。
        OwnerMissing => "owner_missing",
        /// 来源的持有人与登记人不符。
        OwnerMismatch => "owner_mismatch",
        /// 本站已证最后持有人与登记人不符。
        HistoryOwnerMismatch => "history_owner_mismatch",
        /// 同件装备的历史证据冲突。
        HistoryAliasConflict => "history_alias_conflict",
        /// 交付或返还证据不足。
        HistoryUnproven => "history_unproven",
        /// 已有挂单占用或未结拍卖责任。
        EquipmentBusy => "equipment_busy",
        /// 来源未找到装备或读取凭证无效。
        EquipmentNotFound => "equipment_not_found",
        /// 来源拒绝请求。
        SourceRequestRejected => "source_request_rejected",
        /// 来源 HTTP 响应不受支持。
        SourceHttpInvalid => "source_http_invalid",
        /// 来源数据格式错误。
        SourceSchemaInvalid => "source_schema_invalid",
        /// 来源数据形态尚未确认。
        SourceShapeUnconfirmed => "source_shape_unconfirmed",
        /// 来源装备标识不符。
        SourceIdentityMismatch => "source_identity_mismatch",
        /// 来源响应超出预算。
        SourceTooLarge => "source_too_large",
        /// 解析尝试次数耗尽。
        LookupExhausted => "lookup_exhausted",
        /// 本地装备读取凭证不可用。
        CredentialUnavailable => "credential_unavailable",
        /// 买家未付款，装备已收回。
        UnpaidRecovered => "unpaid_recovered",
        /// 买家已付款后退回。
        PaidReturned => "paid_returned",
        /// 活动额度已满。
        QuotaBlocked => "quota_blocked",
        /// 运营止售持续有效。
        OperationsHold => "operations_hold",
        /// 待人工复核。
        Review => "review",
        /// 已成交。
        Sold => "sold",
        /// 撤回件已交还原卖家。
        WithdrawnDelivered => "withdrawn_delivered",
        /// 季界已关闭。
        SeasonClosed => "season_closed",
        /// 已确认丢失。
        LossConfirmed => "loss_confirmed",
    }
}

string_enum! {
    /// 服务器允许本人对挂单执行的 v2 动作。
    pub enum ListingActionV2 {
        /// 修改标价。
        Reprice => "reprice",
        /// 撤回或取消。
        Withdraw => "withdraw",
        /// 从在库下架恢复销售代。
        Relist => "relist",
        /// 签发或换发寄件预约。
        IssueReservation => "issue_reservation",
        /// 公开买家购买。
        Purchase => "purchase",
        /// Admin 下架或设止售。
        AdminDelist => "admin_delist",
        /// Admin 解除止售。
        ReleaseHold => "release_hold",
    }
}

/// 处分代。前三期的解析代从 1 开始，第 4 期的寄售处分代也使用同一安全整数区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingGeneration(i64);

impl<'de> Deserialize<'de> for ListingGeneration {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        positive_version(deserializer).map(Self)
    }
}

impl Serialize for ListingGeneration {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_positive_version(&self.0, serializer)
    }
}

json_object! {
    ()
    /// 回执中的逐件 L：挂单 ID、版本与状态。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ListingStampV2 {
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        pub listing_version: ListingVersion,
        pub state: ListingStateV2,
    } => validate
}

impl ListingStampV2 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.listing_id.is_nil() {
            return Err("挂单标识不得为零 UUID");
        }
        Ok(())
    }
}

json_object! {
    (tag = "kind", rename_all = "snake_case")
    /// 本人或 Admin 可见的 v2 有效视图。
    #[derive(Debug, Clone, PartialEq)]
    pub enum ListingViewV2 {
        /// 尚在解析。
        Resolving { progress: LookupProgress },
        /// 未取得可信快照即终结。
        LookupFailed {
            reason: ListingReasonV2,
            #[serde(deserialize_with = "deserialize_required_option")]
            last_lookup_reason: Option<LookupReason>,
        },
        /// 有安全快照，但未通过发布资格。
        Ineligible { equipment: EquipmentView, reason: ListingReasonV2 },
        /// 非寄售广告正在展示。
        Displayed { equipment: EquipmentView },
        /// 已通过核验，尚未代管。
        AwaitingDeposit { equipment: EquipmentView },
        /// 整封已接受，等待原 Take。
        DepositAccepted { equipment: EquipmentView },
        /// 已代管并上架。
        Available { equipment: EquipmentView },
        /// 已成立 Sale，正在履约。
        SalePending { equipment: EquipmentView },
        /// 在库下架；独立 hold 可以与其他原因并存。
        HeldOffMarket {
            equipment: EquipmentView,
            reason: ListingReasonV2,
            #[serde(deserialize_with = "required_option_uuid")]
            hold_id: Option<Uuid>,
        },
        /// 撤回意愿或 Withdrawal 正在履行。
        WithdrawalPending { equipment: EquipmentView },
        /// 已关闭，保留原发布快照（如果存在）。
        Closed {
            reason: ListingReasonV2,
            #[serde(deserialize_with = "deserialize_required_option")]
            equipment: Option<EquipmentView>,
        },
        /// 已到期，保留原发布快照（如果存在）。
        Expired {
            reason: ListingReasonV2,
            #[serde(deserialize_with = "deserialize_required_option")]
            equipment: Option<EquipmentView>,
        },
    } => validate
}

impl ListingViewV2 {
    fn state_and_reason(&self) -> (ListingStateV2, Option<ListingReasonV2>) {
        match self {
            Self::Resolving { .. } => (ListingStateV2::Resolving, None),
            Self::LookupFailed { reason, .. } | Self::Ineligible { reason, .. } => {
                (ListingStateV2::Rejected, Some(*reason))
            }
            Self::Displayed { .. } => (ListingStateV2::Displayed, None),
            Self::AwaitingDeposit { .. } => (ListingStateV2::AwaitingDeposit, None),
            Self::DepositAccepted { .. } => (ListingStateV2::DepositAccepted, None),
            Self::Available { .. } => (ListingStateV2::Available, None),
            Self::SalePending { .. } => (ListingStateV2::SalePending, None),
            Self::HeldOffMarket { reason, .. } => (ListingStateV2::HeldOffMarket, Some(*reason)),
            Self::WithdrawalPending { .. } => (ListingStateV2::WithdrawalPending, None),
            Self::Closed { reason, .. } => (ListingStateV2::Closed, Some(*reason)),
            Self::Expired { reason, .. } => (ListingStateV2::Expired, Some(*reason)),
        }
    }

    fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::LookupFailed { reason, .. } if !reason.is_lookup_failure() => {
                return Err("解析失败视图的原因不属于未取得快照闭集");
            }
            Self::Ineligible { reason, .. } if !reason.rejected_with_snapshot() => {
                return Err("拒挂视图的原因不属于带快照闭集");
            }
            Self::Ineligible { reason, equipment }
                if equipment.tradeable == (*reason == ListingReasonV2::NotTradeable) =>
            {
                return Err("拒挂原因与装备快照不一致");
            }
            Self::Displayed { equipment }
            | Self::AwaitingDeposit { equipment }
            | Self::DepositAccepted { equipment }
            | Self::Available { equipment }
            | Self::SalePending { equipment }
            | Self::WithdrawalPending { equipment } => equipment.validate_tradeable()?,
            Self::HeldOffMarket { equipment, reason, hold_id } => {
                if !reason.is_hold_reason() {
                    return Err("在库下架视图的原因不属于 held_off_market 闭集");
                }
                if *reason == ListingReasonV2::OperationsHold && hold_id.is_none() {
                    return Err("运营止售必须携带 hold_id");
                }
                if hold_id.is_some_and(|hold_id| hold_id.is_nil()) {
                    return Err("hold_id 不得为零 UUID");
                }
                equipment.validate_tradeable()?;
            }
            Self::Closed { reason, .. } if !reason.is_closed_reason() => {
                return Err("关闭视图的原因不属于关闭闭集");
            }
            Self::Expired { reason, .. } if !reason.is_expiry_reason() => {
                return Err("到期视图的原因不属于到期闭集");
            }
            _ => {}
        }
        if let Self::Closed { equipment, .. } | Self::Expired { equipment, .. } = self
            && let Some(equipment) = equipment
        {
            equipment.validate_tradeable()?;
        }
        Ok(())
    }
}

json_object! {
    ()
    /// v2 公开目录与详情共用的安全摘要。
    #[derive(Debug, Clone, PartialEq)]
    pub struct ListingSummaryV2 {
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        pub version: ListingVersion,
        pub mode: ListingMode,
        pub state: PublicListingStateV2,
        pub world_id: ListingWorld,
        #[serde(deserialize_with = "equipment_id")]
        pub external_equipment_id: String,
        #[serde(deserialize_with = "text::<512, _>")]
        pub name: String,
        #[serde(deserialize_with = "text::<128, _>")]
        pub category: String,
        pub level: EquipmentLevel,
        pub price: ListingPrice,
        pub seller_name: String,
        #[serde(deserialize_with = "timestamp")]
        pub updated_at: Timestamp,
        #[serde(deserialize_with = "timestamp")]
        pub expires_at: Timestamp,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub generation: Option<ListingGeneration>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub effective_reason: Option<ListingReasonV2>,
        pub allowed_actions: Vec<ListingActionV2>,
        pub purchasable: bool,
    } => validate
}

json_object! {
    ()
    /// v2 公开详情：摘要与同一行的安全装备快照。
    #[derive(Debug, Clone, PartialEq)]
    pub struct ListingDetailV2 {
        pub summary: ListingSummaryV2,
        pub equipment: EquipmentView,
    } => validate
}

impl ListingDetailV2 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.summary.name != self.equipment.name
            || self.summary.category != self.equipment.category
            || self.summary.level != self.equipment.level
        {
            return Err("公开详情的装备摘要与快照不一致");
        }
        self.equipment.validate_tradeable()
    }
}

json_object! {
    ()
    /// v2 本人列表与详情共用的完整投影。
    #[derive(Debug, Clone, PartialEq)]
    pub struct OwnedListingV2 {
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
        #[serde(deserialize_with = "deserialize_required_option")]
        pub generation: Option<ListingGeneration>,
        pub stored_state: ListingStateV2,
        pub effective_state: ListingStateV2,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub effective_reason: Option<ListingReasonV2>,
        pub allowed_actions: Vec<ListingActionV2>,
        pub view: ListingViewV2,
    } => validate
}

json_object! {
    (tag = "disposition", rename_all = "snake_case")
    /// 本人 v2 撤回的两种合法回执：代管件登记意愿，未代管件按前三期关闭。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum WithdrawReceiptV2 {
        /// 代管件登记撤回意愿。
        WithdrawalRequested {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            listing_id: Uuid,
            listing_version: ListingVersion,
            state: ListingStateV2,
            #[serde(deserialize_with = "uuid")]
            intent_id: Uuid,
        },
        /// 未代管件直接关闭。
        Closed {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            listing_id: Uuid,
            listing_version: ListingVersion,
            state: ListingStateV2,
        },
    } => validate
}

write_object! {
    /// 本人 v2 撤回或取消请求。不变量 1、6：稳定 UUIDv4 与预期版本。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WithdrawListingV2 {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub expected_version: ListingVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
    }
}

impl WithdrawReceiptV2 {
    fn validate(&self) -> Result<(), &'static str> {
        // 第 4 期三个代管态都只登记撤回意愿：available 转入 withdrawal_pending，
        // deposit_accepted/held_off_market 保留原态；withdrawal_accepted 属第 7 期。
        match self {
            Self::WithdrawalRequested {
                schema_version,
                operation,
                command_id,
                listing_id,
                state,
                intent_id,
                ..
            } => {
                if *schema_version != 2
                    || *operation != ConsignmentOperationV2::Withdraw
                    || !matches!(
                        state,
                        ListingStateV2::DepositAccepted
                            | ListingStateV2::WithdrawalPending
                            | ListingStateV2::HeldOffMarket
                    )
                {
                    return Err("撤回意愿回执的版本、operation 或状态不一致");
                }
                if command_id.is_nil() || listing_id.is_nil() || intent_id.is_nil() {
                    return Err("撤回意愿回执的标识不得为零 UUID");
                }
            }
            Self::Closed { schema_version, operation, command_id, listing_id, state, .. } => {
                if *schema_version != 2
                    || *operation != ConsignmentOperationV2::Withdraw
                    || *state != ListingStateV2::Closed
                {
                    return Err("未代管撤回回执的版本、operation 或状态不一致");
                }
                if command_id.is_nil() || listing_id.is_nil() {
                    return Err("未代管撤回回执的标识不得为零 UUID");
                }
            }
        }
        Ok(())
    }
}
