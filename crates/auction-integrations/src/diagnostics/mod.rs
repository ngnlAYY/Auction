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

pub(crate) fn debug(
    kind: &'static str,
    phase: &'static str,
    error: ErrorProjection,
    context: DiagnosticContext<'_>,
) {
    record(DiagnosticLevel::Debug, kind, phase, error, context);
}
