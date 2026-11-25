//! 恢复处置的领域错误；只携带安全分类，不回显外部输入。

use thiserror::Error;

/// 处置事实自身不成立（调用方装配错误），区别于“待判留案”的业务结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RecoveryDomainError {
    /// 退回腿与世界不匹配（如 main 世界出现 isekai 载体腿）。
    #[error("退回腿与世界不匹配")]
    LegWorldMismatch,
    /// main 世界没有寄售季，不能带换季清空事实。
    #[error("main 世界没有寄售季")]
    SeasonEndedOnMain,
}
