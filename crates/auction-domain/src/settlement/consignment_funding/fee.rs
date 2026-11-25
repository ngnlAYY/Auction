//! 手续费分录：单独记账，不进补偿池；退款只冲回本笔手续费（决定 19）。

use uuid::Uuid;

use super::errors::FundingRuleError;
use crate::consignment::ListingQuote;

/// 手续费行数规则：仅 F > 0 时有 earned 与退款时的 reversed。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeRows {
    pub earned: bool,
    pub reverse_on_full_refund: bool,
}

/// 按冻结报价判定行数；禁止零额占位。
pub fn fee_rows(quote: ListingQuote) -> FeeRows {
    let has_fee = quote.fee > 0;
    FeeRows { earned: has_fee, reverse_on_full_refund: has_fee }
}

/// 已记入的 earned 分录：绑定合同与原收据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeEarned {
    pub contract_id: Uuid,
    pub receipt_source_id: Uuid,
    pub amount: i64,
}

/// 需要写入的 reversed 分录：引用同一 earned、同额、同一退款义务。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeReversal {
    pub contract_id: Uuid,
    pub receipt_source_id: Uuid,
    pub refund_obligation_id: Uuid,
    pub amount: i64,
}

/// 决定全额退款转换时的手续费冲回。
///
/// 保护的不变量：reversed 只能引用本合同的那一条 earned、金额等于冻结 F、且至多一次；
/// F = 0 时不得有 earned，也不产生 reversed。
pub fn reverse_fee(
    contract_id: Uuid,
    quote: ListingQuote,
    earned: Option<FeeEarned>,
    already_reversed: bool,
    refund_obligation_id: Uuid,
) -> Result<Option<FeeReversal>, FundingRuleError> {
    if !fee_rows(quote).reverse_on_full_refund {
        // F = 0：既无 earned 也无 reversed；出现 earned 说明账实不符。
        return if earned.is_none() && !already_reversed {
            Ok(None)
        } else {
            Err(FundingRuleError::FeeBinding)
        };
    }
    let earned = earned.ok_or(FundingRuleError::FeeBinding)?;
    if earned.contract_id != contract_id || earned.amount != quote.fee {
        return Err(FundingRuleError::FeeBinding);
    }
    if already_reversed {
        return Err(FundingRuleError::FeeAlreadyReversed);
    }
    Ok(Some(FeeReversal {
        contract_id,
        receipt_source_id: earned.receipt_source_id,
        refund_obligation_id,
        amount: earned.amount,
    }))
}
