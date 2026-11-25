//! Admin 订单读模型与证据分页；Admin 同时看到买卖双方，金额不按角色省略。

mod evidence;
mod resolve;

use auction_time::Timestamp;
use serde::Deserialize;
use uuid::Uuid;

use crate::DeliveryStageV2;
use crate::LegStageV2;
use crate::ListingPrice;
use crate::ListingWorld;
use crate::OrderKindV2;
use crate::OrderStateV2;
use crate::PaymentStageV2;
use crate::deserialize_required_option;
use crate::dto::public::consignment::order_v2::stages_match;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::text;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::positive_version;
use crate::dto::public::consignment::wire_v2::required_option_uuid;
use crate::dto::public::consignment::wire_v2::serialize_positive_version;

pub use evidence::*;
pub use resolve::*;

/// 键必须出现；值可以为 null，非空值是 RFC 3339 UTC 时刻。
fn optional_timestamp<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Timestamp>, D::Error> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Utc(#[serde(deserialize_with = "timestamp")] Timestamp);

    Option::<Utc>::deserialize(deserializer).map(|value| value.map(|value| value.0))
}

json_object! {
    ()
    /// Admin 订单。买家字段与金额只在成交订单出现；不含预约码、装备 key 与原信正文。
    #[derive(Debug, Clone, PartialEq)]
    pub struct AdminOrderV2 {
        #[serde(deserialize_with = "uuid")]
        pub contract_id: Uuid,
        pub kind: OrderKindV2,
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub equipment_id: Uuid,
        pub world: ListingWorld,
        pub order_state: OrderStateV2,
        pub delivery_stage: DeliveryStageV2,
        pub payment_stage: PaymentStageV2,
        pub payout_stage: LegStageV2,
        pub refund_stage: LegStageV2,
        #[serde(
            deserialize_with = "positive_version",
            serialize_with = "serialize_positive_version"
        )]
        pub version: i64,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub price: Option<ListingPrice>,
        #[serde(deserialize_with = "optional_timestamp")]
        pub deadline_at: Option<Timestamp>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub fee: Option<i64>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub seller_amount: Option<i64>,
        #[serde(deserialize_with = "required_option_uuid")]
        pub buyer_user_id: Option<Uuid>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub buyer_name: Option<String>,
        #[serde(deserialize_with = "uuid")]
        pub seller_user_id: Uuid,
        #[serde(deserialize_with = "text::<128, _>")]
        pub seller_name: String,
        #[serde(deserialize_with = "timestamp")]
        pub updated_at: Timestamp,
    } => validate
}

impl AdminOrderV2 {
    fn validate(&self) -> Result<(), &'static str> {
        match self.kind {
            OrderKindV2::Sale => self.validate_sale()?,
            OrderKindV2::Withdrawal => {
                if self.price.is_some()
                    || self.deadline_at.is_some()
                    || self.fee.is_some()
                    || self.seller_amount.is_some()
                    || self.buyer_user_id.is_some()
                    || self.buyer_name.is_some()
                {
                    return Err("撤回订单不得带价格、期限、金额或买家");
                }
            }
        }
        if !stages_match(
            self.kind,
            self.order_state,
            self.delivery_stage,
            self.payment_stage,
            self.payout_stage,
            self.refund_stage,
        ) {
            return Err("订单阶段组合与订单态矛盾");
        }
        Ok(())
    }

    /// 不变量 6、10：P = F + S，手续费不吞没全部价款，买卖双方不同账号。
    fn validate_sale(&self) -> Result<(), &'static str> {
        let (Some(price), Some(_), Some(fee), Some(seller_amount), Some(buyer), Some(_)) = (
            self.price,
            self.deadline_at,
            self.fee,
            self.seller_amount,
            self.buyer_user_id,
            self.buyer_name.as_deref(),
        ) else {
            return Err("成交订单必须带价格、期限、金额与买家");
        };
        let price = price.get();
        if !(0..price).contains(&fee)
            || ListingPrice::new(seller_amount).is_none()
            || fee + seller_amount != price
        {
            return Err("成交订单金额不守恒：价格必须等于手续费加到手金额");
        }
        if buyer == self.seller_user_id {
            return Err("买家与卖家不得是同一账号");
        }
        Ok(())
    }
}
