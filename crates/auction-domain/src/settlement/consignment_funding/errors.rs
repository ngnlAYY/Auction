//! 寄售资金规则的失败分类；所有违反都失败关闭，不做截断或自动修正。

use thiserror::Error;

/// 资金规则违反的低基数分类，不含金额以外的外部内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FundingRuleError {
    /// 金额为负、为零（不允许为零处）或区间非法。
    #[error("资金金额无效")]
    InvalidAmount,
    /// 已含于旧 opening 的份额超过毛实收。
    #[error("opening 已含份额超过毛实收")]
    OpeningExceedsGross,
    /// 毛实收与价格、世界推出的期望毛额不一致。
    #[error("毛实收与期望毛额不一致")]
    GrossMismatch,
    /// 报价不满足 P = S + F。
    #[error("报价与标价不守恒")]
    QuoteMismatch,
    /// 分账各段合计与毛实收不等。
    #[error("分账合计与毛实收不等")]
    Imbalance,
    /// 同一 opening entry 的份额区间重叠。
    #[error("opening 份额区间重叠")]
    SliceOverlap,
    /// 手续费分录与报价或合同不符。
    #[error("手续费分录与合同不符")]
    FeeBinding,
    /// 本笔手续费已被冲回。
    #[error("手续费已冲回")]
    FeeAlreadyReversed,
    /// 同一本金槽位出现互斥结论。
    #[error("本金结论互斥冲突")]
    PrincipalConflict,
    /// 阻断记录字段无效。
    #[error("opening 阻断记录无效")]
    InvalidBlock,
    /// 该 UID 存在未核清的现金退件，拒绝 opening。
    #[error("存在未核清的现金退件阻断")]
    OpeningBlocked,
}
