//! 第 6 期订单读模型：本人订单列表与详情的角色视图及金额守恒校验。

mod stage;

use auction_time::Timestamp;
use uuid::Uuid;

use crate::deserialize_required_option;
use crate::dto::public::consignment::catalog::ListingPage;
use crate::dto::public::consignment::scalar::ListingPrice;
use crate::dto::public::consignment::states::ListingWorld;
use crate::dto::public::consignment::wire::PRICE_MAX;
use crate::dto::public::consignment::wire::integer;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::optional_timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::positive_version;
use crate::dto::public::consignment::wire_v2::serialize_positive_version;

pub use stage::*;

/// 订单分页：筛选 `role、world、state、page、page_size`，页壳与挂单分页一致。
pub type OrderPageV2 = ListingPage<OrderV2>;

/// 卖方视图手续费：缺省即省略；出现时必须是 0 到价格上限的整数，不接受 null。
fn present_fee<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    integer::<0, PRICE_MAX, D>(deserializer).map(Some)
}

/// 卖方视图到手金额：缺省即省略；出现时必须是 1 到价格上限的整数，不接受 null。
fn present_seller_amount<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    integer::<1, PRICE_MAX, D>(deserializer).map(Some)
}

json_object! {
    ()
    /// 本人订单。金额按角色省略：只有卖方的成交订单带 `fee` 与 `seller_amount`，
    /// 买方与撤回订单不含私有金额。不得含预约码、装备 key、对方账号或原信正文。
    #[derive(Debug, Clone, PartialEq)]
    pub struct OrderV2 {
        #[serde(deserialize_with = "uuid")]
        pub contract_id: Uuid,
        pub kind: OrderKindV2,
        pub role: OrderRoleV2,
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
        pub allowed_actions: Vec<OrderActionV2>,
        #[serde(
            deserialize_with = "positive_version",
            serialize_with = "serialize_positive_version"
        )]
        pub version: i64,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub price: Option<ListingPrice>,
        #[serde(deserialize_with = "optional_timestamp")]
        pub deadline_at: Option<Timestamp>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_fee"
        )]
        pub fee: Option<i64>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_seller_amount"
        )]
        pub seller_amount: Option<i64>,
    } => validate
}

impl OrderV2 {
    fn validate(&self) -> Result<(), &'static str> {
        // 第 6 期买卖双方没有订单命令；放开动作须与第 7 期的授权矩阵一起改。
        if !self.allowed_actions.is_empty() {
            return Err("第 6 期订单不得提供动作");
        }
        match self.kind {
            OrderKindV2::Sale => {
                let (Some(price), Some(_)) = (self.price, self.deadline_at) else {
                    return Err("成交订单必须带价格与付款期限");
                };
                self.validate_amounts(price.get())?;
            }
            OrderKindV2::Withdrawal => {
                if self.role != OrderRoleV2::Seller {
                    return Err("撤回订单只对原卖家可见");
                }
                if self.price.is_some()
                    || self.deadline_at.is_some()
                    || self.fee.is_some()
                    || self.seller_amount.is_some()
                {
                    return Err("撤回订单不得带价格、期限或金额");
                }
            }
        }
        if !stage::stages_match(
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

    /// 不变量 6、10：卖方视图 P = F + S；买方视图不得带手续费与到手金额。
    fn validate_amounts(&self, price: i64) -> Result<(), &'static str> {
        match (self.role, self.fee, self.seller_amount) {
            (OrderRoleV2::Buyer, None, None) => Ok(()),
            (OrderRoleV2::Buyer, _, _) => Err("买方视图不得带手续费或到手金额"),
            (OrderRoleV2::Seller, Some(fee), Some(seller_amount)) => {
                if fee < price && fee + seller_amount == price {
                    Ok(())
                } else {
                    Err("卖方金额不守恒：价格必须等于手续费加到手金额")
                }
            }
            (OrderRoleV2::Seller, _, _) => Err("卖方成交订单必须带手续费与到手金额"),
        }
    }
}
