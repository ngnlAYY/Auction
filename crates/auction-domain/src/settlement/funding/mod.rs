//! 真实现金用途分配规则；冻结经济金额不重算，余额观察不等于自有资金。

use crate::{DomainError, SettlementAmount};

/// 现金账户稳定身份；凭据版本及运行代不参与经济账户键。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FundingAccountKey(String);

impl FundingAccountKey {
    /// 只接受规范正十进制 UID，现金固定为 Main Credits。
    pub fn new(uid: &str) -> Result<Self, DomainError> {
        if uid.starts_with('0')
            || !uid.bytes().all(|byte| byte.is_ascii_digit())
            || uid.parse::<i64>().ok().filter(|value| *value > 0).is_none()
        {
            return Err(DomainError::InvalidSettlementFacts("资金账户 UID 无效"));
        }
        Ok(Self(uid.to_owned()))
    }

    /// 返回经过校验的认证 UID。
    pub fn uid(&self) -> &str {
        &self.0
    }
}

/// 只校验既有冻结 B+C=S+T，不计算新的税或补偿。
#[derive(Debug, Clone, Copy)]
pub struct FundingSplit {
    pub buyer: SettlementAmount,
    pub compensation: SettlementAmount,
    pub seller: SettlementAmount,
    pub tax: SettlementAmount,
}

impl FundingSplit {
    /// 校验来源与用途严格守恒。
    pub fn validate(self) -> Result<Self, DomainError> {
        if self.buyer.checked_add(self.compensation)? != self.seller.checked_add(self.tax)? {
            return Err(DomainError::InvalidSettlementFacts("资金来源与用途不守恒"));
        }
        Ok(self)
    }
}

/// 对账只能授权真实现金扣除既有占用和完整旧责任之后的余量。
pub fn funding_reconciliation_capacity(
    observed: SettlementAmount,
    allocated: SettlementAmount,
    liabilities: SettlementAmount,
    proprietary: SettlementAmount,
    compensation: SettlementAmount,
) -> Result<SettlementAmount, DomainError> {
    observed
        .checked_sub(allocated)?
        .checked_sub(liabilities)?
        .checked_sub(proprietary)?
        .checked_sub(compensation)
}
