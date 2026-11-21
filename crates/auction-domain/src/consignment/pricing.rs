//! 定义挂单标价、费率与版本化报价规则。

use crate::consignment::errors::ListingError;
use crate::consignment::listing::{ListingMode, ListingPrice};

/// 登记时与适用费率一起冻结的算法版本，见合同第 2.2 节。
pub const PRICING_VERSION: u16 = 1;

/// 以基点表示的有效费率。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FeeBps(u16);

impl FeeBps {
    /// 非寄售的固定费率。
    pub const DIRECT: Self = Self(0);
    /// 寄售的配置默认费率，见维护者决定 5 与合同第 6 节。
    pub const DEFAULT_CONSIGNED: Self = Self(500);

    /// 构造 `0..=9999` 基点内的费率。
    pub fn new(bps: i64) -> Result<Self, ListingError> {
        // 不变量 6：拒绝负费率和达到全额的费率，不做截断。
        match u16::try_from(bps) {
            Ok(value @ 0..=9_999) => Ok(Self(value)),
            _ => Err(ListingError::InvalidFee),
        }
    }

    /// 返回整数基点。
    pub const fn value(self) -> u16 {
        self.0
    }

    /// 供登记比较预期费率并冻结适用值；非寄售的费率永远为 0。
    pub const fn for_mode(mode: ListingMode, configured_consigned: Self) -> Self {
        match mode {
            ListingMode::Direct => Self::DIRECT,
            ListingMode::Consigned => configured_consigned,
        }
    }
}

/// 按登记时的算法版本和费率计算的预计到手报价。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingQuote {
    /// 从卖家收入中扣除的预计手续费，单位为 Credits。
    pub fee: i64,
    /// 卖家预计到手的 Credits。
    pub seller_amount: i64,
}

/// 计算 v1 报价，手续费向下取整。
pub fn quote_v1(price: ListingPrice, fee_bps: FeeBps) -> ListingQuote {
    // 不变量 10：前三期仅展示预计到手；成交时才冻结金额。
    // 经校验的标价与费率之积小于 10^13，i64 可精确表示。
    let price = price.value();
    let fee = price * i64::from(fee_bps.value()) / 10_000;
    ListingQuote { fee, seller_amount: price - fee }
}

#[cfg(test)]
mod tests {
    use crate::consignment::errors::ListingError;
    use crate::consignment::listing::{ListingMode, ListingPrice};
    use crate::consignment::pricing::{FeeBps, ListingQuote, PRICING_VERSION, quote_v1};

    #[test]
    fn fee_bps_validates_bounds_and_applies_mode() -> Result<(), ListingError> {
        for bps in [0, 500, 9_999] {
            let fee = FeeBps::new(bps)?;
            assert_eq!(i64::from(fee.value()), bps);
            assert_eq!(FeeBps::for_mode(ListingMode::Direct, fee).value(), 0);
            assert_eq!(FeeBps::for_mode(ListingMode::Consigned, fee), fee);
        }
        for bps in [-1, 10_000, 65_536, i64::MAX] {
            assert_eq!(FeeBps::new(bps), Err(ListingError::InvalidFee));
        }
        assert_eq!(FeeBps::DEFAULT_CONSIGNED.value(), 500);
        assert_eq!(PRICING_VERSION, 1);
        Ok(())
    }

    #[test]
    fn quote_v1_floors_fee_and_keeps_contract_example() -> Result<(), ListingError> {
        for (price, fee, seller_amount) in [(101, 5, 96), (119, 5, 114), (120, 6, 114), (19, 0, 19)]
        {
            assert_eq!(
                quote_v1(ListingPrice::new(price)?, FeeBps::DEFAULT_CONSIGNED),
                ListingQuote { fee, seller_amount },
                "标价 {price}"
            );
        }
        Ok(())
    }

    #[test]
    fn quote_v1_conserves_credits_without_overflow() -> Result<(), ListingError> {
        for price in [1, 19, 101, 10_000, 999_999_999, 1_000_000_000] {
            let price = ListingPrice::new(price)?;
            for bps in [0, 1, 500, 9_999] {
                let quote = quote_v1(price, FeeBps::new(bps)?);
                assert!(quote.fee >= 0);
                assert!(quote.seller_amount > 0);
                assert_eq!(quote.fee + quote.seller_amount, price.value());
            }
        }
        assert_eq!(
            quote_v1(ListingPrice::new(1_000_000_000)?, FeeBps::new(9_999)?),
            ListingQuote { fee: 999_900_000, seller_amount: 100_000 }
        );
        assert_eq!(
            quote_v1(ListingPrice::new(1_000_000_000)?, FeeBps::DIRECT),
            ListingQuote { fee: 0, seller_amount: 1_000_000_000 }
        );
        Ok(())
    }
}
