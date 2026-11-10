//! Admin 人工 Sent 证明对话框。
//!
//! 只允许选择该投递维护流程已写入的 Sent 证据（`mailbox_sweep` 或
//! `sent_reference_sighting`），并把冻结尝试与观察组合成 `auction-contracts` 的闭集请求。
//! 预览无副作用；提交沿用页面命令宿主的稳定 `command_id`，服务端仍会重新判定准入，
//! 不把预览当作成功证明，也不重发邮件或改写原失败命令。
//!
//! 状态、证据选择、预览/确认动作与渲染分别由子模块拥有；对外仍只暴露对话框、
//! 一次性提交、可引用证据与证据种类文案。

mod evidence;
mod preview;
mod render;
mod state;
mod validation;

pub(super) use evidence::selectable_evidence;
pub(super) use render::{SentProofDialog, evidence_kind_label};
pub(super) use state::SentProofSubmission;

#[cfg(test)]
mod tests;
