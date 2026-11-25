//! isekai 载体本金的结论互斥规则。

use super::errors::FundingRuleError;

/// 每 (合同, 载体义务, 原 delivery, 轮) 至多一个本金结论，两者互斥。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalConclusion {
    /// 买家已付款，本金并入收款分账。
    PaymentPrincipal,
    /// 载体被退回，本金经精确 Take 回补。
    RtsPrincipal,
}

/// 在已有结论上落新结论：相同为幂等重放（返回 `false` 表示无新增），相反为冲突。
pub fn resolve_principal_conclusion(
    existing: Option<PrincipalConclusion>,
    requested: PrincipalConclusion,
) -> Result<bool, FundingRuleError> {
    match existing {
        None => Ok(true),
        Some(current) if current == requested => Ok(false),
        Some(_) => Err(FundingRuleError::PrincipalConflict),
    }
}
