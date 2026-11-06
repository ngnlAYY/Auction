//! HTTP 安全诊断：只同步借用框架路由模板，复用既有请求 span 的规范化 request-id。

use std::{cell::Cell, future::Future};

use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticLevel, SafeErrorKind, record_error,
};
use axum::{extract::MatchedPath, http::StatusCode, response::Response};
use tracing::{Instrument, Span};

/// 既定结构校验只产生固定拒绝；不读取或记录传入值。
pub(crate) fn invalid_state<T: ?Sized>(_cause: &T) -> SafeErrorKind {
    SafeErrorKind::DOMAIN_REJECTED
}

pub(crate) fn invalid_header(_cause: &axum::http::header::InvalidHeaderValue) -> SafeErrorKind {
    SafeErrorKind::REQUEST_ENCODING_ERROR
}

pub(crate) fn timeout_kind(_cause: &tokio::time::error::Elapsed) -> SafeErrorKind {
    SafeErrorKind::TIMEOUT
}

pub(crate) fn io_kind(_cause: &std::io::Error) -> SafeErrorKind {
    SafeErrorKind::IO_ERROR
}

pub(crate) fn connection_kind<T: ?Sized>(_cause: &T) -> SafeErrorKind {
    SafeErrorKind::REMOTE_TRANSPORT_ERROR
}

pub(crate) fn join_kind(_cause: &tokio::task::JoinError) -> SafeErrorKind {
    SafeErrorKind::TASK_JOIN
}

/// panic 只记类别，禁止遍历或格式化 payload。
pub(crate) fn panic_kind(_cause: &(dyn std::any::Any + Send)) -> SafeErrorKind {
    SafeErrorKind::TASK_PANIC
}

pub(crate) fn health_kind(cause: &auction_observability::HealthError) -> SafeErrorKind {
    use auction_observability::HealthError;
    match cause {
        HealthError::InvalidCommand | HealthError::InvalidData => SafeErrorKind::DOMAIN_REJECTED,
        HealthError::StaleObservation => SafeErrorKind::EVIDENCE_EXPIRED,
        HealthError::Storage { .. } => SafeErrorKind::REPOSITORY_FAILED,
    }
}

struct RequestState {
    route: Option<MatchedPath>,
    // 只去掉 helper 与最终响应观察的重复日志，不参与状态码、指标或命令判定。
    response_logged: Cell<Option<StatusCode>>,
}

tokio::task_local! { static ROUTE: RequestState; }

/// 只传递观测上下文，不重建请求 future、期限、身份或状态。
pub(crate) async fn scope<F: Future>(route: Option<MatchedPath>, body: F) -> F::Output {
    ROUTE.scope(RequestState { route, response_logged: Cell::new(None) }, body).await
}

/// 升级任务显式继承原请求关联；不生成新的 request-id，不保留请求或正文。
pub(crate) struct RequestDiagnostics {
    route: Option<MatchedPath>,
    span: Span,
}

impl RequestDiagnostics {
    pub(crate) fn capture() -> Self {
        Self {
            route: ROUTE.try_with(|state| state.route.clone()).ok().flatten(),
            span: Span::current(),
        }
    }

    pub(crate) async fn run<F: Future>(self, body: F) -> F::Output {
        scope(self.route, body).instrument(self.span).await
    }
}

fn record(
    level: DiagnosticLevel,
    kind: &'static str,
    phase: &'static str,
    safe_kind: SafeErrorKind,
    context: DiagnosticContext<'_>,
) {
    let recorded = ROUTE.try_with(|state| {
        let route = state.route.as_ref().map_or("__unmatched__", MatchedPath::as_str);
        record_error(
            level,
            safe_kind,
            kind,
            phase,
            DiagnosticContext { route: context.route.or(Some(route)), ..context },
        );
    });
    if recorded.is_err() {
        record_error(
            level,
            safe_kind,
            kind,
            phase,
            DiagnosticContext { route: context.route.or(Some("__background__")), ..context },
        );
    }
}

pub(crate) fn warn(
    kind: &'static str,
    phase: &'static str,
    safe_kind: SafeErrorKind,
    context: DiagnosticContext<'_>,
) {
    record(DiagnosticLevel::Warn, kind, phase, safe_kind, context);
}

pub(crate) fn error(
    kind: &'static str,
    phase: &'static str,
    safe_kind: SafeErrorKind,
    context: DiagnosticContext<'_>,
) {
    record(DiagnosticLevel::Error, kind, phase, safe_kind, context);
}

pub(crate) fn debug(
    kind: &'static str,
    phase: &'static str,
    safe_kind: SafeErrorKind,
    context: DiagnosticContext<'_>,
) {
    record(DiagnosticLevel::Debug, kind, phase, safe_kind, context);
}

/// 捕获的安全原因跨过原同步 mapper；只查看状态码，原响应逐值返回。
pub(crate) fn mapped_response(
    kind: &'static str,
    phase: &'static str,
    safe_kind: SafeErrorKind,
    response: Response,
) -> Response {
    if response.status().is_server_error() {
        let level = match response.status() {
            StatusCode::INTERNAL_SERVER_ERROR => DiagnosticLevel::Error,
            StatusCode::NOT_IMPLEMENTED => DiagnosticLevel::Info,
            _ => DiagnosticLevel::Warn,
        };
        record(level, kind, phase, safe_kind, DiagnosticContext::default());
    }
    response
}

/// helper 和直接构造的 5xx 共用最终响应兜底；不读取错误码、JSON 或响应正文。
pub(crate) fn response_status(status: StatusCode) {
    response_error(status, None);
}

/// 静态错误码仅用于识别未启用的能力，不输出错误码、正文或 DTO。
pub(crate) fn response_error(status: StatusCode, code: Option<&'static str>) {
    if !status.is_server_error() {
        return;
    }
    let fresh = ROUTE.try_with(|state| state.response_logged.replace(Some(status)) != Some(status));
    if matches!(fresh, Ok(false)) {
        return;
    }
    if matches!(code, Some("feature_unavailable" | "feature_not_supported")) {
        record(
            DiagnosticLevel::Info,
            "http_response",
            "respond_unsupported",
            SafeErrorKind::CAPABILITY_UNAVAILABLE,
            DiagnosticContext::default(),
        );
        return;
    }
    let (level, phase, safe_kind) = match status {
        StatusCode::INTERNAL_SERVER_ERROR => {
            (DiagnosticLevel::Error, "respond_internal_error", SafeErrorKind::HTTP_INTERNAL_ERROR)
        }
        StatusCode::NOT_IMPLEMENTED => {
            (DiagnosticLevel::Info, "respond_unsupported", SafeErrorKind::CAPABILITY_UNAVAILABLE)
        }
        StatusCode::BAD_GATEWAY => {
            (DiagnosticLevel::Warn, "respond_bad_gateway", SafeErrorKind::HTTP_BAD_GATEWAY)
        }
        StatusCode::SERVICE_UNAVAILABLE => {
            (DiagnosticLevel::Warn, "respond_unavailable", SafeErrorKind::HTTP_SERVICE_UNAVAILABLE)
        }
        StatusCode::GATEWAY_TIMEOUT => {
            (DiagnosticLevel::Warn, "respond_timeout", SafeErrorKind::TIMEOUT)
        }
        _ => (DiagnosticLevel::Error, "respond_server_error", SafeErrorKind::HTTP_SERVER_ERROR),
    };
    record(level, "http_response", phase, safe_kind, DiagnosticContext::default());
}
