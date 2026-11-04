//! 由调用循环拥有的固定类别日志去重；只保留计时与计数，不保存错误正文。

mod diagnostics;
pub use diagnostics::{DiagnosticLogOwner, DiagnosticLogWindows};
mod panic;
pub(crate) use panic::PanicSafeJson;
pub use panic::install_safe_panic_hook;

use std::time::Duration;

use tokio::time::Instant;

const WINDOW: Duration = Duration::from_secs(60);
const SUMMARY_TARGET: &str = "auction_observability::log_dedup";

/// 同一固定 target/kind 的错误日志窗口；每个键由调用方持有一个实例。
/// 首条由调用方原样输出，后续重复只计数；必须在 Tokio `select!` 中持续轮询
/// `flush_when_due`，保证错误停止后也能在窗口到期时输出汇总。丢弃实例会补齐剩余计数。
pub struct RateLimitedLog {
    target: &'static str,
    kind: &'static str,
    window: Option<LogWindow>,
    phase: Option<&'static str>,
    safe_kind: Option<&'static str>,
}

struct LogWindow {
    ends_at: Instant,
    suppressed_count: u64,
}

impl RateLimitedLog {
    /// 使用固定、无秘密的 target/kind 建立去重键；不得传入错误正文或动态业务标识。
    /// 窗口固定为 60 秒，避免为少数日志引入新的运行配置；汇总统一使用 WARN。
    pub fn new(target: &'static str, kind: &'static str) -> Self {
        Self { target, kind, window: None, phase: None, safe_kind: None }
    }

    /// 诊断窗口额外保留固定阶段/安全类别；不保存首条业务标识。
    fn diagnostic(kind: &'static str, phase: &'static str, safe_kind: &'static str) -> Self {
        Self {
            target: "auction::diagnostics",
            kind,
            phase: Some(phase),
            safe_kind: Some(safe_kind),
            window: None,
        }
    }

    /// 返回是否应输出本次原始日志；调用方的日志级别和安全字段保持不变。
    /// 调用前先用原事件的 `tracing::enabled!` 检查过滤，避免为不可见日志生成汇总。
    /// 专用汇总 target 被过滤时直接放行原日志，避免只抑制却看不到计数。
    pub fn should_log(&mut self) -> bool {
        if !tracing::enabled!(target: SUMMARY_TARGET, tracing::Level::WARN) {
            self.flush();
            return true;
        }

        let now = Instant::now();
        if self.window.as_ref().is_some_and(|window| now >= window.ends_at) {
            self.flush();
        }
        if let Some(window) = &mut self.window {
            // 日志去重不能溢出而终止 Worker；即使计数饱和，窗口首条仍正常输出。
            window.suppressed_count = window.suppressed_count.saturating_add(1);
            return false;
        }

        self.window = Some(LogWindow { ends_at: now + WINDOW, suppressed_count: 0 });
        true
    }

    /// 等待当前窗口到期并汇总；没有窗口时保持 pending，不周期性唤醒。
    /// 取消等待不会改变窗口或计数，可直接用于循环中的 `tokio::select!`。
    pub async fn flush_when_due(&mut self) {
        let Some(ends_at) = self.window.as_ref().map(|window| window.ends_at) else {
            return std::future::pending().await;
        };
        tokio::time::sleep_until(ends_at).await;
        self.flush();
    }

    fn deadline(&self) -> Option<Instant> {
        self.window.as_ref().map(|window| window.ends_at)
    }

    fn flush_if_due(&mut self) {
        if self.deadline().is_some_and(|deadline| Instant::now() >= deadline) {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let Some(window) = self.window.take().filter(|window| window.suppressed_count > 0) else {
            return;
        };
        // 汇总只携带固定键与计数，不继承调用方 span，也不留存首条的动态字段。
        tracing::warn!(
            target: SUMMARY_TARGET,
            parent: None,
            log_target = self.target,
            kind = self.kind,
            phase = self.phase,
            safe_kind = self.safe_kind,
            suppressed_count = window.suppressed_count,
            window_seconds = WINDOW.as_secs(),
            window_complete = Instant::now() >= window.ends_at,
            "重复错误日志窗口汇总"
        );
    }
}

impl Drop for RateLimitedLog {
    fn drop(&mut self) {
        self.flush();
    }
}
