//! 集成错误的同步安全投影；日志入口不接收错误、URL、响应、凭据或远端身份。

use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticLevel, SafeErrorKind, record_error,
};

mod projection;
pub(crate) use projection::{IntegrationDiagnostic, mooglemail_kind};

#[derive(Clone, Copy)]
pub(crate) struct ErrorProjection {
    kind: SafeErrorKind,
    context: DiagnosticContext<'static>,
}

impl ErrorProjection {
    fn new(kind: SafeErrorKind) -> Self {
        Self { kind, context: DiagnosticContext::default() }
    }
}

/// 已知闭集原因没有底层错误对象时使用；不接受任意标签文本。
pub(crate) fn category(kind: SafeErrorKind) -> ErrorProjection {
    ErrorProjection::new(kind)
}

/// 类型化错误只在此投影；不调用 Display、Debug、source 或读取负载文本。
pub(crate) fn project<E: IntegrationDiagnostic + ?Sized>(error: &E) -> ErrorProjection {
    error.integration_diagnostic()
}

fn record(
    level: DiagnosticLevel,
    kind: &'static str,
    phase: &'static str,
    error: ErrorProjection,
    context: DiagnosticContext<'_>,
) {
    let context = DiagnosticContext {
        world: context.world.or(error.context.world),
        write_started: context.write_started.or(error.context.write_started),
        evidence_id: context.evidence_id.or(error.context.evidence_id),
        ..context
    };
    record_error(level, error.kind, kind, phase, context);
}

pub(crate) fn warn(
    kind: &'static str,
    phase: &'static str,
    error: ErrorProjection,
    context: DiagnosticContext<'_>,
) {
    record(DiagnosticLevel::Warn, kind, phase, error, context);
}

pub(crate) fn error(
    kind: &'static str,
    phase: &'static str,
    error: ErrorProjection,
    context: DiagnosticContext<'_>,
) {
    record(DiagnosticLevel::Error, kind, phase, error, context);
}

pub(crate) fn info(
    kind: &'static str,
    phase: &'static str,
    error: ErrorProjection,
    context: DiagnosticContext<'_>,
) {
    record(DiagnosticLevel::Info, kind, phase, error, context);
}

pub(crate) fn debug(
    kind: &'static str,
    phase: &'static str,
    error: ErrorProjection,
    context: DiagnosticContext<'_>,
) {
    record(DiagnosticLevel::Debug, kind, phase, error, context);
}

/// 供 `map_err` 使用：以 WARN 记录原错误的安全投影（默认上下文）后丢弃原错误，
/// 返回 `fold` 构造的折叠错误。需要 world/write_started 等上下文的调用点仍直接调用 [`warn`]。
pub(crate) fn warn_map<E: IntegrationDiagnostic, R>(
    kind: &'static str,
    phase: &'static str,
    fold: impl FnOnce() -> R,
) -> impl FnOnce(E) -> R {
    record_map(DiagnosticLevel::Warn, kind, phase, fold)
}

/// 同 [`warn_map`]，级别为 ERROR。
pub(crate) fn error_map<E: IntegrationDiagnostic, R>(
    kind: &'static str,
    phase: &'static str,
    fold: impl FnOnce() -> R,
) -> impl FnOnce(E) -> R {
    record_map(DiagnosticLevel::Error, kind, phase, fold)
}

/// 同 [`warn_map`]，级别为 DEBUG。
pub(crate) fn debug_map<E: IntegrationDiagnostic, R>(
    kind: &'static str,
    phase: &'static str,
    fold: impl FnOnce() -> R,
) -> impl FnOnce(E) -> R {
    record_map(DiagnosticLevel::Debug, kind, phase, fold)
}

fn record_map<E: IntegrationDiagnostic, R>(
    level: DiagnosticLevel,
    kind: &'static str,
    phase: &'static str,
    fold: impl FnOnce() -> R,
) -> impl FnOnce(E) -> R {
    move |cause| {
        record(level, kind, phase, project(&cause), DiagnosticContext::default());
        fold()
    }
}

#[cfg(test)]
pub(crate) mod recording;

#[cfg(test)]
mod tests {
    use auction_application::core::diagnostics::SafeErrorKind;

    use super::recording::records;
    use super::{error_map, warn_map};

    /// 映射闭包只在出错时记录一次：级别、固定 kind/phase 与原错误的安全分类；成功值原样通过。
    #[test]
    fn map_helpers_record_projection_once_and_fold_error() {
        records("map_helpers_test");
        let folded: Result<u8, &str> = Err(ring::error::Unspecified).map_err(warn_map(
            "map_helpers_test",
            "warn_phase",
            || "folded",
        ));
        assert_eq!(folded, Err("folded"));
        let passed: Result<u8, &str> = Ok::<u8, ring::error::Unspecified>(7).map_err(error_map(
            "map_helpers_test",
            "ok_phase",
            || "unused",
        ));
        assert_eq!(passed, Ok(7));
        let failed: Result<u8, u8> = Err(uuid::Uuid::try_parse("x").expect_err("非法 UUID"))
            .map_err(error_map("map_helpers_test", "error_phase", || 9));
        assert_eq!(failed, Err(9));

        assert_eq!(
            records("map_helpers_test"),
            vec![
                ("warn", "map_helpers_test", "warn_phase", SafeErrorKind::CRYPTOGRAPHIC_ERROR),
                ("error", "map_helpers_test", "error_phase", SafeErrorKind::DOMAIN_REJECTED),
            ]
        );
    }
}
