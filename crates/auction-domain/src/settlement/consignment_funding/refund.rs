//! 已证退款原因下，卖款与手续费向买家退款的转换金额。

use super::distribution::check_quote;
use super::errors::FundingRuleError;
use crate::consignment::{ListingPrice, ListingQuote};

/// 退款转换：S + F = P 全部转为买家退款。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefundConversion {
    pub from_seller: i64,
    pub from_fee: i64,
    pub to_refund: i64,
}

/// 由冻结报价得到转换金额；F = 0 时 `from_fee` 为 0，退款仍为一笔 P。
/// 保护金额守恒：S + F != P 的报价拒绝，避免退款额与卖款加手续费账面失配。
pub fn refund_conversion(
    price: ListingPrice,
    quote: ListingQuote,
) -> Result<RefundConversion, FundingRuleError> {
    check_quote(price, quote)?;
    Ok(RefundConversion {
        from_seller: quote.seller_amount,
        from_fee: quote.fee,
        to_refund: price.value(),
    })
}

/// 已付待原件时买家最大退款负债是 P，由受限 S + F 覆盖；不是 S + P，也不把互斥的卖款与退款相加。
pub const fn buyer_refund_liability(price: ListingPrice) -> i64 {
    price.value()
}
