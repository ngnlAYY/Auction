//! 成交撤销后的补偿池冲正规则。
//!
//! 结算按权责发生制把附加费计入补偿池、把补偿从池中支出；成交随后被撤销（买方未付款、
//! 退回卖家或已付款后退款）时，这两笔从未真正兑现的金额必须冲回。冲正是新的追加事实，
//! 不改写原结算明细、补偿账本或冻结策略；改变冲正计算结果必须新增规则版本。

use crate::{DomainError, SettlementAmount};

/// 当前冲正规则版本。
pub const COMPENSATION_REVERSAL_RULE_VERSION: u32 = 1;

/// 一次成交撤销对补偿池的冲正结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompensationReversal {
    /// 冲正前的池余额。
    pub opening_balance: SettlementAmount,
    /// 退回池中的名义补偿（全额）。
    pub compensation_returned: SettlementAmount,
    /// 实际从池中冲回的附加费。
    pub tax_reversed: SettlementAmount,
    /// 余额不足、无法冲回的附加费；只在余额耗尽时为正。
    pub tax_shortfall: SettlementAmount,
    /// 冲正后的池余额。
    pub closing_balance: SettlementAmount,
}

impl CompensationReversal {
    /// 按 v1 规则计算冲正：先退回补偿，再冲回附加费；附加费冲回以当前余额为上限，
    /// 池余额永不为负。附加费与补偿都为零时没有冲正事实，返回 `None`。
    pub fn calculate(
        opening_balance: SettlementAmount,
        tax: SettlementAmount,
        compensation: SettlementAmount,
    ) -> Result<Option<Self>, DomainError> {
        if tax.value() == 0 && compensation.value() == 0 {
            return Ok(None);
        }
        let available = opening_balance.checked_add(compensation)?;
        let tax_reversed = tax.min(available);
        let reversal = Self {
            opening_balance,
            compensation_returned: compensation,
            tax_reversed,
            tax_shortfall: tax.checked_sub(tax_reversed)?,
            closing_balance: available.checked_sub(tax_reversed)?,
        };
        Ok(Some(reversal))
    }

    /// 对池余额的带符号净变化：退回的补偿减去冲回的附加费。
    pub fn amount(self) -> i64 {
        self.compensation_returned.value() - self.tax_reversed.value()
    }
}
