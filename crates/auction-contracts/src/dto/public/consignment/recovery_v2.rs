//! 卖家本人重列与损失回执的 v2 契约（第 7 期 7C-3、7C-4）。
//!
//! 损失回执只给编号、状态与回执时刻：不含私有说明、证据来源与摘要、装备 key 或任何金额字段，
//! 没有付款或发行能力（决定 22，不变量 6、7）。

use auction_time::Timestamp;
use uuid::Uuid;

use crate::LossCaseState;
use crate::dto::public::consignment::listing_v2::ListingGeneration;
use crate::dto::public::consignment::listing_v2::ListingStateV2;
use crate::dto::public::consignment::scalar::ListingPrice;
use crate::dto::public::consignment::scalar::ListingVersion;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::command_uuid;
use crate::dto::public::consignment::wire_v2::string_enum;
use crate::dto::public::consignment::write::write_object;

string_enum! {
    /// 重列回执的 operation 固定值。
    pub enum RelistOperationV2 {
        /// 卖家重列。
        Relist => "relist",
    }
}

string_enum! {
    /// 重列回执的处置固定值。
    pub enum RelistDispositionV2 {
        /// 已建新销售代并回到可购买。
        Relisted => "relisted",
    }
}

write_object! {
    /// 本人重列请求。不变量 1、6：稳定 UUIDv4 与预期版本。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RelistListingV2 {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub expected_version: ListingVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
    }
}

json_object! {
    ()
    /// `200 relisted` 回执：挂单回到 `available`，带沿用的价格与新处分代序号。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RelistReceiptV2 {
        /// 固定 v2。
        pub schema_version: u16,
        pub operation: RelistOperationV2,
        pub disposition: RelistDispositionV2,
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        pub listing_version: ListingVersion,
        pub state: ListingStateV2,
        pub price: ListingPrice,
        pub generation: ListingGeneration,
    } => validate
}

impl RelistReceiptV2 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 2 || self.state != ListingStateV2::Available {
            return Err("重列回执的版本或状态不一致");
        }
        // 重列是上代之后的新代，最小为 2。
        if self.generation.get() < 2 {
            return Err("重列回执的处分代必须大于 1");
        }
        if self.command_id.is_nil() || self.listing_id.is_nil() {
            return Err("重列回执的标识不得为零 UUID");
        }
        Ok(())
    }
}

json_object! {
    ()
    /// 卖家本人可查的损失回执：编号、当前状态与最近一份回执。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SellerLossCaseV2 {
        #[serde(deserialize_with = "uuid")]
        pub position_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub loss_case_id: Uuid,
        pub state: LossCaseState,
        #[serde(deserialize_with = "timestamp")]
        pub recorded_at: Timestamp,
        #[serde(deserialize_with = "uuid")]
        pub receipt_id: Uuid,
    } => validate
}

impl SellerLossCaseV2 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.position_id.is_nil() || self.loss_case_id.is_nil() || self.receipt_id.is_nil() {
            return Err("损失回执的标识不得为零 UUID");
        }
        Ok(())
    }
}
