//! 装备损失的人工责任承担；仅记账资金不构成 COD 实收或买方补偿完成证明。

use crate::{DomainError, FundingSplit};

/// 按冻结结算分配操作者承担的买方责任和既有补偿，不重新计算卖家金额。
pub fn equipment_loss_funding(
    split: FundingSplit,
    record_only: bool,
    buyer_compensation_external: bool,
) -> Result<FundingSplit, DomainError> {
    if !record_only || !buyer_compensation_external || split.buyer.value() == 0 {
        return Err(DomainError::InvalidSettlementFacts(
            "装备损失处置需要仅记账账户及外部买方补偿责任声明",
        ));
    }
    split.validate()
}
