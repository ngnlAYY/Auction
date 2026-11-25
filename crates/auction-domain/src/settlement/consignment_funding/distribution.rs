//! 成交分账与收款过渡用途的三段计划。

use super::errors::FundingRuleError;
use super::receipt_split::CodReceiptSplit;
use crate::consignment::{ListingPrice, ListingQuote};
use crate::settlement::SettlementWorld;

/// 资金账户模式；寄售只用 main Credits。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundingMode {
    StrictCash,
    RecordOnly,
}

/// isekai 载体本金固定为 1 Credit。
const CARRIER_PRINCIPAL: i64 = 1;

/// main 付款毛额 = P；isekai 载体毛额 = P + 1。
pub const fn expected_gross(world: SettlementWorld, price: i64) -> i64 {
    match world {
        SettlementWorld::Main => price,
        SettlementWorld::Isekai => price + CARRIER_PRINCIPAL,
    }
}

/// 报价必须满足 P = S + F，且 F >= 0、S > 0；否则失败关闭（不变量 6）。
pub(super) fn check_quote(
    price: ListingPrice,
    quote: ListingQuote,
) -> Result<(), FundingRuleError> {
    if quote.fee < 0 || quote.seller_amount <= 0 || quote.fee + quote.seller_amount != price.value()
    {
        return Err(FundingRuleError::QuoteMismatch);
    }
    Ok(())
}

/// 一笔成交的最终分账；`fee` 为 0 时不产生 fee 项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaleDistribution {
    pub seller: i64,
    pub fee: i64,
    pub principal: i64,
}

/// 由冻结报价与世界得到 S / F / 本金。
pub fn sale_distribution(
    world: SettlementWorld,
    _price: ListingPrice,
    quote: ListingQuote,
) -> SaleDistribution {
    let principal = match world {
        SettlementWorld::Main => 0,
        SettlementWorld::Isekai => CARRIER_PRINCIPAL,
    };
    SaleDistribution { seller: quote.seller_amount, fee: quote.fee, principal }
}

/// 过渡用途的转出项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitOutput {
    Seller(i64),
    Fee(i64),
    Principal { amount: i64, mode: FundingMode },
}

impl TransitOutput {
    const fn amount(&self) -> i64 {
        match *self {
            Self::Seller(amount) | Self::Fee(amount) | Self::Principal { amount, .. } => amount,
        }
    }
}

/// 收款过渡用途的三段计划：opening 借出 A、本笔 source 发行 U，转出 S / F / 本金。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitPlan {
    pub opening_borrow: i64,
    pub new_issue: i64,
    pub outputs: Vec<TransitOutput>,
}

impl TransitPlan {
    /// 转出合计。
    pub fn outputs_total(&self) -> i64 {
        self.outputs.iter().map(TransitOutput::amount).sum()
    }
}

/// 计算 COD 收款计划。保护不变量 6：borrow + issue == gross == outputs 合计，否则失败关闭。
pub fn plan_cod_funding(
    world: SettlementWorld,
    mode: FundingMode,
    price: ListingPrice,
    quote: ListingQuote,
    split: CodReceiptSplit,
) -> Result<TransitPlan, FundingRuleError> {
    check_quote(price, quote)?;
    if split.gross() != expected_gross(world, price.value()) {
        return Err(FundingRuleError::GrossMismatch);
    }
    let distribution = sale_distribution(world, price, quote);
    let mut outputs = vec![TransitOutput::Seller(distribution.seller)];
    // F = 0 时合同与投影明确无 fee 项，不落零额占位。
    if distribution.fee > 0 {
        outputs.push(TransitOutput::Fee(distribution.fee));
    }
    if distribution.principal > 0 {
        outputs.push(TransitOutput::Principal { amount: distribution.principal, mode });
    }
    let plan = TransitPlan {
        opening_borrow: split.opening_included(),
        new_issue: split.new_issue(),
        outputs,
    };
    if plan.opening_borrow + plan.new_issue != split.gross()
        || plan.outputs_total() != split.gross()
    {
        return Err(FundingRuleError::Imbalance);
    }
    Ok(plan)
}
