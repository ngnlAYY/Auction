//! 维护诊断与预算错误：远端失败只记录安全分类，预算耗尽统一返回可分类的仓库错误。

use std::time::Instant;

use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, DiagnosticLevel, diagnostic_world,
    record_error,
};
use crate::ports::core::RepositoryError;

/// 记录一次未持久化的远端读取或丢弃失败；不清除本轮已获证据，只保留安全分类与消息。
pub(super) fn record_unpersisted_error(
    operation: &'static str,
    world_id: &str,
    error: &ApplicationError,
) {
    record_error(
        DiagnosticLevel::Warn,
        error.diagnostic_kind(),
        "mail_maintenance",
        operation,
        DiagnosticContext {
            world: diagnostic_world(world_id),
            disposition: Some(DiagnosticDisposition::Unpersisted),
            ..Default::default()
        },
    );
}

pub(super) fn check_deadline(deadline: Instant) -> Result<(), ApplicationError> {
    if Instant::now() >= deadline {
        return Err(repository_error("MoogleMail 维护总预算耗尽"));
    }
    Ok(())
}

pub(super) fn repository_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
