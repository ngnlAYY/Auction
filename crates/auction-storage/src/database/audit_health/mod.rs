//! 进程内审计写入失败计数；审计 INSERT 失败会连事务一起回滚，这里只留下饱和的进程级信号。
//! 计数不持久化、不含 SQL 或错误原文，由进程心跳安全投影读取。

use std::sync::atomic::{AtomicI64, Ordering};

static AUDIT_FAILURE_COUNT: AtomicI64 = AtomicI64::new(0);

/// 为审计 SQL 的返回值附加失败观测，不改变原始错误及其控制流。
pub(crate) trait AuditWriteResult<T> {
    fn record_audit_failure(self) -> Self;
}

impl<T, E> AuditWriteResult<T> for Result<T, E> {
    fn record_audit_failure(self) -> Self {
        if self.is_err() {
            // Rust 1.99 的新名 try_update 在 MSRV 1.94.1 尚未稳定，例外仅限这一调用。
            #[allow(deprecated)]
            let _ =
                AUDIT_FAILURE_COUNT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    Some(value.saturating_add(1))
                });
        }
        self
    }
}

/// 返回本进程已观察到的审计写入失败饱和计数。
pub(crate) fn failure_count() -> i64 {
    AUDIT_FAILURE_COUNT.load(Ordering::Relaxed)
}
