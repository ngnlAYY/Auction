//! 外部客户端共用的凭据运行时 owner 探测：探测在独立任务中完成，不随调用方取消。
//!
//! owner 端口规定丢弃未完成的 `check()` future 会永久撤销所有权。外层预算（读取轮次、
//! 证据截止、Worker item timeout）只能放弃等待结果，不能取消探测本身；否则一次无关的预算
//! 到期就会撤销 owner 并迫使 Worker 排空重启。探测自身仍受 [`OWNER_PROBE_TIMEOUT`] 约束：
//! 超过上限时由探测任务内的超时取消，按原设计永久撤销 owner。

use std::sync::Arc;
use std::time::Duration;

use auction_application::admin::AdminCredentialRuntimeOwnership;
use auction_application::core::ApplicationError;
use thiserror::Error;

/// 单次探测的硬上限，覆盖持锁连接的排队与探测查询。
const OWNER_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// owner 探测失败；调用方一律按凭据不可用失败关闭，变体只区分诊断阶段。
#[derive(Debug, Error)]
pub(crate) enum OwnerProbeError {
    /// 探测超过硬上限并被取消，owner 已按端口契约撤销。
    #[error("凭据 owner 探测超时")]
    Timeout(#[source] tokio::time::error::Elapsed),
    /// owner 已失效或探测查询失败。
    #[error("凭据 owner 探测失败")]
    Failed(#[source] ApplicationError),
    /// 探测任务因运行时关闭或 panic 未完成；未完成的探测同样撤销 owner。
    #[error("凭据 owner 探测任务中止")]
    Aborted(#[source] tokio::task::JoinError),
}

impl OwnerProbeError {
    /// 按调用方的既有阶段名区分“探测过慢”与“owner 失效”，不改变失败关闭的处理。
    pub(crate) fn phase(&self, timeout: &'static str, failed: &'static str) -> &'static str {
        match self {
            Self::Timeout(..) => timeout,
            Self::Failed(..) | Self::Aborted(..) => failed,
        }
    }
}

/// 在独立任务中执行一次 owner 探测；调用方被取消时只丢弃等待句柄，探测照常完成并归还
/// 持锁连接。成功只证明探测时 owner 仍有效，调用方仍须在外部请求前后各自复核。
pub(crate) async fn probe_owner(
    owner: &Arc<dyn AdminCredentialRuntimeOwnership>,
) -> Result<(), OwnerProbeError> {
    let owner = Arc::clone(owner);
    let probe =
        tokio::spawn(async move { tokio::time::timeout(OWNER_PROBE_TIMEOUT, owner.check()).await });
    probe
        .await
        .map_err(OwnerProbeError::Aborted)?
        .map_err(OwnerProbeError::Timeout)?
        .map_err(OwnerProbeError::Failed)
}

/// 各调用点测试共用的可控 owner 替身。
#[cfg(test)]
pub(crate) mod fixture {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use auction_application::admin::AdminCredentialRuntimeOwnership;
    use auction_application::auction::ApplicationFuture;
    use auction_application::core::ApplicationError;
    use auction_application::settlement::MailRuntimeBinding;
    use tokio::sync::Notify;

    /// 探测挂起直到放行；与存储 owner 相同，未完成的探测 future 被丢弃即永久失效。
    #[derive(Default)]
    pub(crate) struct GatedOwnership {
        invalid: AtomicBool,
        completed: AtomicBool,
        fail: bool,
        open: bool,
        release: Notify,
        finished: Notify,
    }

    impl GatedOwnership {
        /// 探测立即报告 owner 失效，但不改变 sticky 状态。
        pub(crate) fn failing() -> Self {
            Self { fail: true, ..Self::default() }
        }

        /// 探测无需放行即成功；供不关心探测时序的调用点测试。
        pub(crate) fn open() -> Self {
            Self { open: true, ..Self::default() }
        }

        /// 放行挂起的探测，并等待它在后台完成。
        pub(crate) async fn release_and_wait(&self) {
            let finished = self.finished.notified();
            self.release.notify_one();
            tokio::time::timeout(Duration::from_secs(5), finished)
                .await
                .expect("探测应在放行后完成");
        }

        /// 是否有探测完整执行到结束。
        pub(crate) fn completed(&self) -> bool {
            self.completed.load(Ordering::SeqCst)
        }
    }

    /// 探测 future 未完成就被丢弃时撤销 owner。
    struct CancellationGuard<'a> {
        owner: &'a GatedOwnership,
        completed: bool,
    }

    impl Drop for CancellationGuard<'_> {
        fn drop(&mut self) {
            if !self.completed {
                self.owner.invalid.store(true, Ordering::SeqCst);
            }
        }
    }

    impl AdminCredentialRuntimeOwnership for GatedOwnership {
        fn check(&self) -> ApplicationFuture<'_, ()> {
            let guard = CancellationGuard { owner: self, completed: false };
            Box::pin(async move {
                let mut guard = guard;
                if self.fail || !self.is_valid() {
                    guard.completed = true;
                    return Err(ApplicationError::CapabilityUnavailable("fixture owner"));
                }
                if !self.open {
                    self.release.notified().await;
                }
                guard.completed = true;
                self.completed.store(true, Ordering::SeqCst);
                self.finished.notify_one();
                Ok(())
            })
        }

        fn is_valid(&self) -> bool {
            !self.invalid.load(Ordering::SeqCst)
        }

        fn mail_runtime_binding(&self) -> Option<MailRuntimeBinding> {
            self.is_valid().then_some(MailRuntimeBinding { generation: 1 })
        }

        fn revoke_internal_use(&self) {
            self.invalid.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use auction_application::admin::AdminCredentialRuntimeOwnership;

    use super::fixture::GatedOwnership;
    use super::{OWNER_PROBE_TIMEOUT, OwnerProbeError, probe_owner};

    #[tokio::test]
    async fn caller_cancellation_does_not_cancel_the_probe() {
        let gated = Arc::new(GatedOwnership::default());
        let owner: Arc<dyn AdminCredentialRuntimeOwnership> = gated.clone();

        // 外层预算先到期：只放弃等待，探测仍在独立任务中挂起。
        let outer = tokio::time::timeout(Duration::from_millis(1), probe_owner(&owner)).await;
        assert!(outer.is_err(), "外层预算应先到期");
        assert!(owner.is_valid(), "外层取消不得撤销 owner");

        gated.release_and_wait().await;
        assert!(gated.completed());
        assert!(owner.is_valid(), "放行后的探测正常完成，owner 保持有效");
    }

    #[tokio::test]
    async fn probe_exceeding_its_bound_still_revokes_the_owner() {
        let gated = Arc::new(GatedOwnership::default());
        let owner: Arc<dyn AdminCredentialRuntimeOwnership> = gated.clone();

        let started = tokio::time::Instant::now();
        let error = probe_owner(&owner).await.expect_err("挂起的探测必须超时");
        assert!(matches!(error, OwnerProbeError::Timeout(..)));
        assert_eq!(error.phase("timeout", "failed"), "timeout");
        assert!(started.elapsed() >= OWNER_PROBE_TIMEOUT);
        assert!(!owner.is_valid(), "超过探测上限仍按原设计撤销 owner");
        assert!(!gated.completed());
    }

    #[tokio::test]
    async fn owner_failure_is_reported_as_failed() {
        let owner: Arc<dyn AdminCredentialRuntimeOwnership> = Arc::new(GatedOwnership::failing());

        let error = probe_owner(&owner).await.expect_err("owner 失效必须失败关闭");
        assert!(matches!(error, OwnerProbeError::Failed(..)));
        assert_eq!(error.phase("timeout", "failed"), "failed");
    }
}
