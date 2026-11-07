//! Public/Admin 原服务 future 持有的诊断门；随原生命周期刷新和关闭，不创建后台任务。

use std::{future::Future, sync::Arc};

use auction_application::core::diagnostics::{
    DiagnosticSink, SafeDiagnostic, diagnostic_enabled, install_diagnostic_sink,
};

use super::DiagnosticLogWindows;

struct ProcessDiagnosticSink(Arc<DiagnosticLogWindows>);

impl DiagnosticSink for ProcessDiagnosticSink {
    fn record(&self, diagnostic: &SafeDiagnostic<'_>) {
        if diagnostic_enabled(diagnostic) {
            self.0.record(diagnostic);
        }
    }
}

/// 原角色服务拥有的日志窗口；不安装 Worker observer，不累加 Worker 未持久化指标。
pub struct DiagnosticLogOwner(Arc<DiagnosticLogWindows>);

impl DiagnosticLogOwner {
    /// tracing 初始化后安装一次；窗口仅保存固定类别，不保存借用的路由或业务标识。
    pub fn install() -> Self {
        let windows = DiagnosticLogWindows::new();
        install_diagnostic_sink(Arc::new(ProcessDiagnosticSink(Arc::clone(&windows))));
        Self(windows)
    }

    /// 同时轮询原 future 和日志截止；不改变原超时、取消、返回值或停止信号。
    pub async fn run<F: Future>(&self, future: F) -> F::Output {
        tokio::pin!(future);
        loop {
            tokio::select! {
                biased;
                result = &mut future => return result,
                () = self.0.flush_when_due() => {}
            }
        }
    }
}

impl Drop for DiagnosticLogOwner {
    fn drop(&mut self) {
        self.0.close();
    }
}
