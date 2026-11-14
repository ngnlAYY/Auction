//! 维护诊断与预算错误：远端失败只记录安全分类，预算耗尽统一返回可分类的仓库错误。

use std::time::Instant;

use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, DiagnosticLevel, diagnostic_world,
    record_error,
};
use crate::ports::core::RepositoryError;

/// 记录一次未持久化的远端读取或丢弃失败；只记安全分类，不清除本轮已获证据。
pub(super) fn record_unpersisted_error(
    operation: &'static str,
    world_id: &str,
    error: &ApplicationError,
) {
    // 账号忙是正常的退避信号；其余维护读取失败仍按 WARN 保留。
    let level = match error.diagnostic_kind().label() {
        "remote_busy" | "remote_account_busy" => DiagnosticLevel::Info,
        _ => DiagnosticLevel::Warn,
    };
    record_error(
        level,
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
