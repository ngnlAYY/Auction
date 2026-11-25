//! 受限权利的领域错误。
//!
//! 错误只携带安全分类，不回显定位符、挑战码或会话令牌的任何输入片段。

use thiserror::Error;

/// 带前缀随机量文本的格式故障。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TextFault {
    /// 前缀不是约定版本。
    #[error("前缀无效")]
    Prefix,
    /// 长度不符。
    #[error("长度无效")]
    Length,
    /// 含字母表以外的字符或填充。
    #[error("字符或填充无效")]
    Alphabet,
    /// 不是唯一规范编码。
    #[error("不是规范编码")]
    NonCanonical,
}

/// 受限权利领域拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RightsError {
    /// 权利定位符格式无效。
    #[error("权利定位符格式无效：{0}")]
    Locator(TextFault),
    /// 会话令牌格式无效。
    #[error("会话令牌格式无效：{0}")]
    SessionToken(TextFault),
    /// 挑战码长度不符。
    #[error("挑战码长度无效")]
    CodeLength,
    /// 挑战码含字母表以外的字符。
    #[error("挑战码字符无效")]
    CodeAlphabet,
    /// 挑战码不是规范编码。
    #[error("挑战码不是规范编码")]
    CodeNonCanonical,
    /// 挑战虚拟、错误、过期、已消费或失败次数用尽；各原因同形，不可区分。
    #[error("挑战无效")]
    ChallengeInvalid,
    /// 会话已过原截止。
    #[error("权利会话已过期")]
    SessionExpired,
    /// 浏览器或 IP 的频率预算用尽。
    #[error("请求过于频繁")]
    BudgetExceeded,
}
