//! 测试专用的进程级诊断记录替身。诊断门每个进程只能安装一次，crate 内需要断言诊断的用例
//! 都经此共用同一个记录器，并按各自的 kind 过滤；并行用例共享 kind 时只能断言“包含”。

use std::sync::{Arc, Mutex, OnceLock};

use auction_application::core::diagnostics::{
    DiagnosticLevel, DiagnosticSink, SafeDiagnostic, SafeErrorKind, install_diagnostic_sink,
};

/// 一条已记录诊断：级别、kind、phase 与安全错误类别。
pub(crate) type Record = (&'static str, &'static str, &'static str, SafeErrorKind);

struct RecordingSink(Mutex<Vec<Record>>);

impl DiagnosticSink for RecordingSink {
    fn record(&self, diagnostic: &SafeDiagnostic<'_>) {
        let level = match diagnostic.level {
            DiagnosticLevel::Error => "error",
            DiagnosticLevel::Warn => "warn",
            DiagnosticLevel::Info => "info",
            DiagnosticLevel::Debug => "debug",
        };
        self.0.lock().expect("记录锁").push((
            level,
            diagnostic.kind,
            diagnostic.phase,
            diagnostic.safe_kind,
        ));
    }
}

/// 首次调用时安装记录器，返回迄今所有 kind 等于参数的诊断。用例须在触发诊断之前先调用一次，
/// 否则安装前产生的诊断不会被记录。
pub(crate) fn records(kind: &str) -> Vec<Record> {
    static SINK: OnceLock<Arc<RecordingSink>> = OnceLock::new();
    let sink = SINK.get_or_init(|| {
        let sink = Arc::new(RecordingSink(Mutex::new(Vec::new())));
        install_diagnostic_sink(Arc::clone(&sink) as Arc<dyn DiagnosticSink>);
        sink
    });
    sink.0.lock().expect("记录锁").iter().filter(|record| record.1 == kind).copied().collect()
}
