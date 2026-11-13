//! 角色服务生命周期持有的有界诊断窗口；只缓存固定键和计数，不缓存事件或业务标识。

mod owner;
pub use owner::DiagnosticLogOwner;

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use auction_application::core::diagnostics::{
    DiagnosticSink, SafeDiagnostic, emit_safe_diagnostic,
};
use tokio::sync::Notify;

use super::RateLimitedLog;

type Key = (&'static str, &'static str, &'static str);
const MAX_KEYS: usize = 512;

/// 复用 RateLimitedLog；进程只安装一个窄诊断门，原循环负责及时 flush。
/// 窗口只是可丢弃的观测缓存，锁损坏或容量不足均放行日志，不影响业务。
pub struct DiagnosticLogWindows {
    windows: Mutex<HashMap<Key, RateLimitedLog>>,
    changed: Notify,
    active: AtomicBool,
}

impl DiagnosticLogWindows {
    /// 由角色的长期 owner 建立，随后安装窄诊断门，不创建独立任务。
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            windows: Mutex::new(HashMap::new()),
            changed: Notify::new(),
            active: AtomicBool::new(true),
        })
    }

    /// 待到最近窗口截止后汇总；等待可取消，不持锁跨 await。
    pub async fn flush_when_due(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let deadline = {
                let windows = match self.windows.lock() {
                    Ok(windows) => windows,
                    Err(poison) => poison.into_inner(),
                };
                windows.values().filter_map(RateLimitedLog::deadline).min()
            };
            match deadline {
                Some(deadline) => tokio::select! {
                    () = &mut changed => continue,
                    () = tokio::time::sleep_until(deadline) => {
                        let mut windows = match self.windows.lock() {
                            Ok(windows) => windows,
                            Err(poison) => poison.into_inner(),
                        };
                        for window in windows.values_mut() { window.flush_if_due(); }
                        windows.retain(|_, window| window.deadline().is_some());
                        return;
                    }
                },
                None => changed.await,
            }
        }
    }

    /// 在原排空完成后汇总并停止抑制；不延后停止通知或取消业务 future。
    pub fn close(&self) {
        self.active.store(false, Ordering::Release);
        let mut windows = match self.windows.lock() {
            Ok(windows) => windows,
            Err(poison) => poison.into_inner(),
        };
        windows.clear();
        self.changed.notify_one();
    }
}

fn tracing_level(level: auction_application::core::diagnostics::DiagnosticLevel) -> tracing::Level {
    use auction_application::core::diagnostics::DiagnosticLevel;
    match level {
        DiagnosticLevel::Error => tracing::Level::ERROR,
        DiagnosticLevel::Warn => tracing::Level::WARN,
        DiagnosticLevel::Info => tracing::Level::INFO,
        DiagnosticLevel::Debug => tracing::Level::DEBUG,
    }
}

impl DiagnosticSink for DiagnosticLogWindows {
    fn record(&self, diagnostic: &SafeDiagnostic<'_>) {
        let allowed = if self.active.load(Ordering::Acquire) {
            let mut windows = match self.windows.lock() {
                Ok(windows) => windows,
                Err(_) => {
                    emit_safe_diagnostic(diagnostic);
                    return;
                }
            };
            // 过期窗口可丢弃，容量不能被已经停止的故障永久占满。
            windows.retain(|_, window| window.deadline().is_some());
            let key = (diagnostic.kind, diagnostic.phase, diagnostic.safe_kind.label());
            if windows.len() >= MAX_KEYS && !windows.contains_key(&key) {
                true
            } else {
                let window = windows.entry(key).or_insert_with(|| {
                    RateLimitedLog::diagnostic(
                        diagnostic.kind,
                        diagnostic.phase,
                        diagnostic.safe_kind.label(),
                    )
                });
                let allowed = window.should_log(tracing_level(diagnostic.level));
                self.changed.notify_one();
                allowed
            }
        } else {
            true
        };
        // 不持去重锁写日志；subscriber/OTLP 的行为不能锁住其他 owner。
        if allowed {
            emit_safe_diagnostic(diagnostic);
        }
    }
}
