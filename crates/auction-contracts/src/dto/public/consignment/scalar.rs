//! 寄售写入与投影共用的受约束整数，不变量 6、10。

use serde::Deserialize;
use serde::Serialize;

use crate::dto::public::consignment::wire::FEE_BPS_MAX;
use crate::dto::public::consignment::wire::PRICE_MAX;
use crate::dto::public::consignment::wire::SAFE_INTEGER_MAX;
use crate::dto::public::consignment::wire::integer;
use crate::dto::public::consignment::wire::serialize_integer;

macro_rules! listing_integer {
    ($(#[$attr:meta])* $name:ident, $min:expr, $max:expr) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name(i64);

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                integer::<{ $min }, { $max }, _>(deserializer).map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serialize_integer::<{ $min }, { $max }, _>(&self.0, serializer)
            }
        }

        impl $name {
            /// 整数超出合同范围时返回空。
            pub const fn new(value: i64) -> Option<Self> {
                if value >= $min && value <= $max { Some(Self(value)) } else { None }
            }

            /// 返回线协议上的整数值。
            pub const fn get(self) -> i64 {
                self.0
            }
        }
    };
}

listing_integer!(
    /// 以 main Credits 计的标价，范围为 1 到十亿。
    ListingPrice,
    1,
    PRICE_MAX
);
listing_integer!(
    /// 从 1 开始、浏览器可精确表示的事实版本。
    ListingVersion,
    1,
    SAFE_INTEGER_MAX
);
listing_integer!(
    /// 万分比费率，范围为 0 到 9999。
    ListingFeeBps,
    0,
    FEE_BPS_MAX
);
listing_integer!(
    /// 冻结的计价策略版本，前三期固定为 1。
    ListingPricingVersion,
    1,
    1
);

#[cfg(test)]
mod tests {
    use super::ListingFeeBps;
    use super::ListingPrice;
    use super::ListingPricingVersion;
    use super::ListingVersion;

    #[test]
    fn invalid_scalars_cannot_be_emitted_even_from_inside_the_module() {
        assert!(serde_json::to_string(&ListingPrice(1_000_000_001)).is_err());
        assert!(serde_json::to_string(&ListingVersion(0)).is_err());
        assert!(serde_json::to_string(&ListingFeeBps(10_000)).is_err());
        assert!(serde_json::to_string(&ListingPricingVersion(0)).is_err());
    }
}
