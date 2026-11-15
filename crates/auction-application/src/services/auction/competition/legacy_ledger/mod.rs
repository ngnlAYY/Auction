//! 命令账本中竞价命令结果旧 wire 的防腐层。
//! 旧实例会严格读取 `OK + BidResult/ProxyResult`，并校验 UUID byte-array、确定性事实 ID、
//! 事实顺序和完整代理转换历史。这里集中完成旧 wire 与内部 `CompetitionPlan` 的双向映射，
//! 避免旧 DTO 扩散到领域层、HTTP 契约或存储适配器。
//!
//! 子模块按职责拆分：`ids` 冻结确定性 ID、`bid`/`proxy`/`proxy_result` 承载旧 wire DTO 与
//! 校验、`decode`/`encode` 做双向映射、`replay` 补回历史代理响应投影。

mod bid;
mod decode;
mod encode;
mod ids;
mod proxy;
mod proxy_result;
mod replay;
#[cfg(test)]
mod tests;

pub(super) use decode::replay;
pub(super) use encode::encode;
pub(super) use ids::deterministic_id;
pub(super) use replay::replay_accepted;

use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::auction::CompetitionProxyCommandOutcome;
use crate::ports::core::CompetitionTransactionError;

/// 竞价命令账本已发布的成功结果码；字节兼容、不可修改，回放按它选择旧 wire 解码（见
/// `competition/replay.rs`）。
pub(super) const LEGACY_RESULT_CODE: &str = "OK";

/// 新命令写账本和返回 HTTP 时共用的同事务冻结结果。
pub(super) struct EncodedCommandResult {
    pub payload: Vec<u8>,
    pub proxy: Option<CompetitionProxyCommandOutcome>,
}

pub(super) fn legacy_payload_error() -> ApplicationError {
    CompetitionTransactionError::Failed("旧版竞价命令账本结果损坏".to_owned()).into()
}

/// 旧 wire 字段在 `phase` 转换失败：记录安全分类后折叠为统一的账本损坏错误。
/// phase 必须是源码固定阶段名；需要 lot/命令上下文的调用点仍直接调用 `record_error`。
fn ledger_error<E: DiagnosticError>(phase: &'static str) -> impl FnOnce(E) -> ApplicationError {
    move |error| {
        record_error(
            DiagnosticLevel::Error,
            error.diagnostic_kind(),
            "competition_ledger",
            phase,
            DiagnosticContext::default(),
        );
        legacy_payload_error()
    }
}
