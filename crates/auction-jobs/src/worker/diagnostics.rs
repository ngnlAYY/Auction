//! Worker 本地错误投影；不格式化 join 的 panic payload 或超时 future。

use std::{
    future::Future,
    sync::{Arc, OnceLock},
};

use auction_application::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticLevel, DiagnosticSink, SafeDiagnostic,
    SafeErrorKind, diagnostic_enabled, diagnostic_world, install_diagnostic_sink, record_error,
};
use auction_observability::{
    DiagnosticLogWindows, WorkerLoop, WorkerRecorder, scope_database_worker_loop,
};

/// 超时只有固定类别，不读取底层 future。
pub(crate) fn timeout_kind(_error: &tokio::time::error::Elapsed) -> SafeErrorKind {
    SafeErrorKind::TIMEOUT
}

/// 不读取任务 panic payload；正常排空取消由调用 owner 排除。
pub(crate) fn join_kind(_error: &tokio::task::JoinError) -> SafeErrorKind {
    SafeErrorKind::TASK_JOIN
}

/// 仅增加子任务 panic 观测；原 join 日志与未持久化 task_join 计数仍由调用点负责。
pub(crate) fn observe_task_panic(kind: &'static str, error: &tokio::task::JoinError) {
    if error.is_cancelled() || !error.is_panic() {
        return;
    }
    // 指标先于日志过滤和去重；不读取 payload，不形成持久处置或改变合并结果。
    let _ = CONTEXT.try_with(|context| context.metrics.observe_task_panic(context.loop_kind));
    self::error(kind, "item_panic", SafeErrorKind::TASK_PANIC, DiagnosticContext::default());
}

/// 依赖、超时和未确认结果的固定 WARN 入口。
pub(crate) fn warn(
    kind: &'static str,
    phase: &'static str,
    safe_kind: SafeErrorKind,
    context: DiagnosticContext,
) {
    record_error(DiagnosticLevel::Warn, safe_kind, kind, phase, context);
}

/// 持久处置失败或内部不变量的固定 ERROR 入口。
pub(crate) fn error(
    kind: &'static str,
    phase: &'static str,
    safe_kind: SafeErrorKind,
    context: DiagnosticContext,
) {
    record_error(DiagnosticLevel::Error, safe_kind, kind, phase, context);
}

/// 正常等待或能力未启用的固定 INFO 入口。
pub(crate) fn info(
    kind: &'static str,
    phase: &'static str,
    safe_kind: SafeErrorKind,
    context: DiagnosticContext,
) {
    record_error(DiagnosticLevel::Info, safe_kind, kind, phase, context);
}

/// 可丢弃提示回退的固定 DEBUG 入口。
pub(crate) fn debug(
    kind: &'static str,
    phase: &'static str,
    safe_kind: SafeErrorKind,
    context: DiagnosticContext,
) {
    record_error(DiagnosticLevel::Debug, safe_kind, kind, phase, context);
}

/// 未确认持久处置的 WARN；指标由 sink 在过滤与去重前累计。
pub(crate) fn unpersisted_error(
    kind: &'static str,
    phase: &'static str,
    safe_kind: SafeErrorKind,
    mut context: DiagnosticContext,
) {
    context.disposition = Some(DiagnosticDisposition::Unpersisted);
    warn(kind, phase, safe_kind, context);
}

#[derive(Clone)]
struct WorkerDiagnosticContext {
    metrics: Arc<dyn WorkerRecorder>,
    loop_kind: WorkerLoop,
}
tokio::task_local! { static CONTEXT: WorkerDiagnosticContext; }

/// 同一循环身份传给诊断与连接观测；不改变预算、取消、返回值或持久化。
pub(crate) async fn scope<F: Future>(
    metrics: Arc<dyn WorkerRecorder>,
    loop_kind: WorkerLoop,
    future: F,
) -> F::Output {
    scope_database_worker_loop(
        loop_kind,
        CONTEXT.scope(WorkerDiagnosticContext { metrics, loop_kind }, future),
    )
    .await
}

/// JoinSet 子任务显式继承观测 owner；Tokio task-local 不会自动跨 spawn。
pub(crate) fn inherit<F: Future>(future: F) -> impl Future<Output = F::Output> {
    let context = CONTEXT.try_with(Clone::clone).ok();
    async move {
        match context {
            Some(context) => scope(context.metrics, context.loop_kind, future).await,
            None => future.await,
        }
    }
}

/// 仅在未确认持久化处置的最终分支调用；不能在每次返回 Err 时自动累计。
pub(crate) fn unpersisted(kind: SafeErrorKind) {
    let _ = CONTEXT
        .try_with(|context| context.metrics.observe_unpersisted_error(context.loop_kind, kind));
}

struct WorkerDiagnosticSink {
    windows: Arc<DiagnosticLogWindows>,
}
impl DiagnosticSink for WorkerDiagnosticSink {
    fn record(&self, diagnostic: &SafeDiagnostic) {
        // 计数先于过滤与去重；普通应用拒绝和已持久处置不带此标记。
        if matches!(diagnostic.context.disposition, Some(DiagnosticDisposition::Unpersisted)) {
            unpersisted(diagnostic.safe_kind);
        }
        if diagnostic_enabled(diagnostic) {
            let mut diagnostic = *diagnostic;
            let _ = CONTEXT.try_with(|context| {
                diagnostic.context.worker_loop = Some(context.loop_kind.label());
                if diagnostic.context.world.is_none() {
                    diagnostic.context.world = match context.loop_kind {
                        WorkerLoop::MailMain
                        | WorkerLoop::MailMaintenanceMain
                        | WorkerLoop::MailReadObservationMain => diagnostic_world("main"),
                        WorkerLoop::MailIsekai
                        | WorkerLoop::MailMaintenanceIsekai
                        | WorkerLoop::MailReadObservationIsekai => diagnostic_world("isekai"),
                        _ => None,
                    };
                }
            });
            self.windows.record(&diagnostic);
        }
    }
}
static WINDOWS: OnceLock<Arc<DiagnosticLogWindows>> = OnceLock::new();

/// PM 和主 Worker 共享一个长期窗口；原主循环刷新与关闭，不产生新后台任务。
pub(crate) fn install() -> Arc<DiagnosticLogWindows> {
    WINDOWS
        .get_or_init(|| {
            let windows = DiagnosticLogWindows::new();
            install_diagnostic_sink(Arc::new(WorkerDiagnosticSink { windows: windows.clone() }));
            windows
        })
        .clone()
}

/// 取消主 future 时也收口窗口；不依赖全局 Arc 的最终析构。
pub(crate) struct DiagnosticLogGuard(Arc<DiagnosticLogWindows>);

impl DiagnosticLogGuard {
    pub(crate) fn new(windows: Arc<DiagnosticLogWindows>) -> Self {
        Self(windows)
    }
}

impl Drop for DiagnosticLogGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}
