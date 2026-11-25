//! 退件规则的解析错误。

use thiserror::Error;

/// 退件规则的闭集解析错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ReturnRuleError {
    /// 退回腿代码不在闭集内。
    #[error("退回腿代码未知")]
    UnknownLeg,
}
