//! 买家购买的请求、回执与确定拒绝码。

use uuid::Uuid;

use crate::dto::public::consignment::listing_v2::ListingStateV2;
use crate::dto::public::consignment::order_v2::OrderStateV2;
use crate::dto::public::consignment::scalar::ListingPrice;
use crate::dto::public::consignment::scalar::ListingVersion;
use crate::dto::public::consignment::states::ListingWorld;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::command_uuid;
use crate::dto::public::consignment::wire_v2::positive_version;
use crate::dto::public::consignment::wire_v2::serialize_positive_version;
use crate::dto::public::consignment::wire_v2::string_enum;
use crate::dto::public::consignment::write::write_object;

string_enum! {
    /// 购买回执的 operation 固定值。
    pub enum PurchaseOperationV2 {
        /// 买家购买。
        Purchase => "purchase",
    }
}

string_enum! {
    /// 购买回执的首次成功处置固定值。
    pub enum PurchaseDispositionV2 {
        /// 已购买，买家获得排他交割权。
        Purchased => "purchased",
    }
}

string_enum! {
    /// 购买可返回的确定拒绝码与结果未知码。
    ///
    /// `outcome_unknown` 只表示提交结果不可判；504、断线与解码失败没有可用码，
    /// 客户端同样必须保留原 actor、目标、输入与命令 ID 原样重放。
    pub enum PurchaseRejectionCodeV2 {
        /// 同命令 ID 的首次请求与原请求不同。
        CommandIdConflict => "command_id_conflict",
        /// 挂单版本已过期。
        StaleVersion => "stale_version",
        /// 标价与预期不符。
        PriceChanged => "price_changed",
        /// 挂单已不可购买。
        ListingNotActive => "listing_not_active",
        /// 装备已被承诺。
        EquipmentCommitted => "equipment_committed",
        /// 代管取得或持有链证据不足。
        CustodyUnproven => "custody_unproven",
        /// 运营止售有效。
        OperationsHold => "operations_hold",
        /// 季界已预停或关闭。
        SeasonClosed => "season_closed",
        /// 买家与卖家是同一本地账号。
        SelfPurchase => "self_purchase",
        /// 认证 Bot 不能作为当事人。
        BotParty => "bot_party",
        /// 当事人不具备资格或有待执行处罚。
        ParticipantIneligible => "participant_ineligible",
        /// 确定写前已知资金能力不可用。
        FundingUnavailable => "funding_unavailable",
        /// 寄售首次禁用。
        ConsignmentDisabled => "consignment_disabled",
        /// 提交结果未知。
        OutcomeUnknown => "outcome_unknown",
    }
}

impl PurchaseRejectionCodeV2 {
    /// 与该码配对的 HTTP 状态。
    pub const fn http_status(self) -> i64 {
        match self {
            Self::CommandIdConflict
            | Self::StaleVersion
            | Self::PriceChanged
            | Self::ListingNotActive
            | Self::EquipmentCommitted
            | Self::CustodyUnproven
            | Self::OperationsHold
            | Self::SeasonClosed => 409,
            Self::SelfPurchase | Self::BotParty | Self::ParticipantIneligible => 403,
            Self::ConsignmentDisabled => 404,
            Self::FundingUnavailable | Self::OutcomeUnknown => 503,
        }
    }

    /// 只有该码要求按原输入重放并先查命令账本。
    pub const fn is_outcome_unknown(self) -> bool {
        matches!(self, Self::OutcomeUnknown)
    }

    /// 只有确定写前的 `funding_unavailable` 可在客户端白名单中视为可信 503。
    pub const fn is_trusted_before_write(self) -> bool {
        matches!(self, Self::FundingUnavailable)
    }
}

write_object! {
    /// 购买请求。不变量 1、6：稳定 UUIDv4、预期版本与预期价同时绑定。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct PurchaseRequestV2 {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub expected_version: ListingVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
        pub expected_price: ListingPrice => ("serde::Deserialize::deserialize", "必须为 1 到十亿的整数"),
    }
}

json_object! {
    ()
    /// `201 purchased` 回执。只含本次成交的公开事实，不含卖家、预约或装备 key。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct PurchaseReceiptV2 {
        /// 固定 v2。
        pub schema_version: u16,
        pub operation: PurchaseOperationV2,
        pub disposition: PurchaseDispositionV2,
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        pub listing_version: ListingVersion,
        pub state: ListingStateV2,
        #[serde(deserialize_with = "uuid")]
        pub contract_id: Uuid,
        #[serde(
            deserialize_with = "positive_version",
            serialize_with = "serialize_positive_version"
        )]
        pub contract_version: i64,
        #[serde(deserialize_with = "uuid")]
        pub equipment_id: Uuid,
        pub world: ListingWorld,
        pub price: ListingPrice,
        pub order_state: OrderStateV2,
    } => validate
}

impl PurchaseReceiptV2 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 2 {
            return Err("购买回执的 schema_version 必须为 2");
        }
        // 购买提交即进入 sale_pending，订单从 awaiting_payment 开始。
        if self.state != ListingStateV2::SalePending
            || self.order_state != OrderStateV2::AwaitingPayment
        {
            return Err("购买回执的挂单状态或订单态不一致");
        }
        Ok(())
    }

    /// 不变量 1、6：回执必须回答该请求——命令 ID 与价格逐字相同，挂单版本向前推进。
    /// 重放返回原回执，所以这里不要求版本恰为预期版本加一。
    /// 返回 `Err` 时调用方必须把结果视为 `outcome_unknown`，只能原样重放同一请求。
    pub fn answers(&self, request: &PurchaseRequestV2) -> Result<(), &'static str> {
        if self.command_id != request.command_id {
            return Err("购买回执的命令 ID 与请求不符");
        }
        if self.price != request.expected_price {
            return Err("购买回执的价格与预期价不符");
        }
        if self.listing_version.get() <= request.expected_version.get() {
            return Err("购买回执的挂单版本没有越过预期版本");
        }
        Ok(())
    }
}
