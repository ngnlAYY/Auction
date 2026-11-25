//! 派发规则的失败分类。

use thiserror::Error;

/// 派发轮次与冻结身份的规则错误；一律失败关闭，不静默修正。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DispatchRuleError {
    /// 首轮不能在已有轮次之上再建。
    #[error("该义务已有派发轮次，不能再建首轮")]
    FirstRoundExists,
    /// 新轮必须有上一轮。
    #[error("新轮缺少上一轮")]
    MissingPriorRound,
    /// 新轮必须引用封闭上一轮的退件接管事实。
    #[error("新轮缺少封闭上一轮的退件接管引用")]
    MissingClosure,
    /// 轮次号溢出或为零。
    #[error("派发轮次号无效")]
    InvalidRound,
    /// 冻结身份缺字段或金额为负。
    #[error("派发冻结身份无效")]
    InvalidIdentity,
}
