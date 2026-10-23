//! 规则页面的反馈状态；已提交意图由页级 PendingCommands 唯一拥有。

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RuleNotice {
    Validated,
    Published,
    ActionError(String),
    Conflict,
}
