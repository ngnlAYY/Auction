//! 寄售合同与义务图的领域错误；只携带固定的安全说明，不回显身份文本或金额。

use thiserror::Error;

use crate::DomainError;

/// 合同条款、策略冻结或义务图校验失败的原因。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConsignmentContractError {
    /// 合同核心的标识、世界、代或冻结身份无效。
    #[error("寄售合同核心无效：{0}")]
    Core(&'static str),
    /// 成交条款（买家、金额、冻结策略或资金账户）无效。
    #[error("寄售成交条款无效：{0}")]
    Sale(&'static str),
    /// 撤回条款无效，例如携带了金额类字段。
    #[error("寄售撤回条款无效：{0}")]
    Withdrawal(&'static str),
    /// 冻结策略超出允许范围。
    #[error("寄售结算策略无效：{0}")]
    Policy(&'static str),
    /// 义务图与固定形状不符。
    #[error("寄售义务图与合同不符：{0}")]
    Graph(&'static str),
    /// 单条义务不满足结算义务矩阵。
    #[error(transparent)]
    Obligation(#[from] DomainError),
}
