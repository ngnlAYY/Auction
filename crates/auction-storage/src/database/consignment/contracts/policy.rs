//! 成交冻结策略与资金账户列的闭集映射；越界值失败关闭，不截断。

use auction_domain::consignment::{ConsignmentSalePolicy, FundingAccountingMode};

/// 资金账户记账方式的持久化名称。
pub(super) const fn funding_mode_name(mode: FundingAccountingMode) -> &'static str {
    match mode {
        FundingAccountingMode::StrictCash => "strict_cash",
        FundingAccountingMode::RecordOnly => "record_only",
    }
}

/// 解析记账方式；未知值返回 `None`，由调用方失败关闭。
pub(super) fn parse_funding_mode(value: &str) -> Option<FundingAccountingMode> {
    match value {
        "strict_cash" => Some(FundingAccountingMode::StrictCash),
        "record_only" => Some(FundingAccountingMode::RecordOnly),
        _ => None,
    }
}

/// 数据库 smallint 列的冻结策略；已持久化的值读回时必须再过领域校验。
#[derive(Debug, Clone, Copy)]
pub(super) struct PolicyColumns {
    pub(super) payment_deadline_days: i16,
    pub(super) reminder_limit: i16,
    pub(super) cod_return_limit: i16,
    pub(super) isekai_refusal_limit: i16,
    pub(super) isekai_refusal_window_days: i16,
}

impl PolicyColumns {
    /// 领域策略的列值；超出 smallint 范围时失败关闭，不截断（不变量 6）。
    pub(super) fn from_policy(
        policy: &ConsignmentSalePolicy,
    ) -> Result<Self, std::num::TryFromIntError> {
        Ok(Self {
            payment_deadline_days: i16::try_from(policy.payment_deadline_days)?,
            reminder_limit: i16::from(policy.reminder_limit),
            cod_return_limit: i16::from(policy.cod_return_limit),
            isekai_refusal_limit: i16::from(policy.isekai_refusal_limit),
            isekai_refusal_window_days: i16::try_from(policy.isekai_refusal_window_days)?,
        })
    }

    /// 还原并校验领域策略；任何越界或固定常量被改写都返回 `None`。
    pub(super) fn into_policy(self) -> Option<ConsignmentSalePolicy> {
        let policy = ConsignmentSalePolicy {
            payment_deadline_days: u16::try_from(self.payment_deadline_days).ok()?,
            reminder_limit: u8::try_from(self.reminder_limit).ok()?,
            cod_return_limit: u8::try_from(self.cod_return_limit).ok()?,
            isekai_refusal_limit: u8::try_from(self.isekai_refusal_limit).ok()?,
            isekai_refusal_window_days: u16::try_from(self.isekai_refusal_window_days).ok()?,
        };
        policy.validate().ok()?;
        Some(policy)
    }
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::ConsignmentSalePolicy;

    use super::PolicyColumns;

    /// 越界的策略值必须报错，不得饱和成 i16::MAX 写库。
    #[test]
    fn from_policy_rejects_out_of_range_values() {
        let mut policy = ConsignmentSalePolicy::new(7, 2, 2).expect("合法策略");
        assert!(PolicyColumns::from_policy(&policy).is_ok());
        policy.payment_deadline_days = u16::MAX;
        assert!(PolicyColumns::from_policy(&policy).is_err());
        policy.payment_deadline_days = 1;
        policy.isekai_refusal_window_days = u16::MAX;
        assert!(PolicyColumns::from_policy(&policy).is_err());
    }
}
