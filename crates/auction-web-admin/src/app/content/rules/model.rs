//! 规则页面的本地命令和反馈状态。

use uuid::Uuid;

use crate::api::AdminRulesCandidate;

#[derive(Debug, Clone)]
pub(super) struct RuleCommand {
    pub(super) command_id: Uuid,
    pub(super) expected_revision: i64,
    pub(super) draft: AdminRulesCandidate,
    pub(super) signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RuleNotice {
    Validated,
    Published,
    ActionError(String),
    Conflict,
}
