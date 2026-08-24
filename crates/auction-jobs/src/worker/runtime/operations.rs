//! Operations alert 三类周期循环的运行时编排。
//!
//! 扫描、分发和对账彼此隔离，单轮失败只影响自身下一次调度，不阻塞其他 Worker owner。

use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) enum OperationsAlertLoopAction {
    Scan,
    Dispatch,
    Reconcile,
}

impl OperationsAlertLoopAction {
    const fn loop_kind(self) -> WorkerLoop {
        match self {
            Self::Scan => WorkerLoop::OperationsAlertScan,
            Self::Dispatch => WorkerLoop::OperationsAlertDispatch,
            Self::Reconcile => WorkerLoop::OperationsAlertReconcile,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Dispatch => "dispatch",
            Self::Reconcile => "reconcile",
        }
    }

    const fn timeout(self, config: OperationsAlertRuntimeConfig) -> std::time::Duration {
        match self {
            Self::Scan => config.scan_timeout,
            Self::Dispatch => config.dispatch_timeout,
            Self::Reconcile => config.reconcile_timeout,
        }
    }
}

/// 独立运行一个 operations alert owner；单轮错误只影响自身下一次轮询。
pub(super) async fn run_operations_alert_loop<C>(
    runtime: Arc<OperationsAlertRuntime>,
    clock: Arc<C>,
    metrics: Arc<dyn WorkerRecorder>,
    action: OperationsAlertLoopAction,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    loop {
        let started = std::time::Instant::now();
        let now = clock.now();
        let attempt = tokio::time::timeout(action.timeout(runtime.config), async {
            match action {
                OperationsAlertLoopAction::Scan => runtime.scan(now).await.map(|()| None),
                OperationsAlertLoopAction::Dispatch => runtime.dispatch(now).await.map(|()| None),
                OperationsAlertLoopAction::Reconcile => {
                    runtime.reconcile(now, runtime.config.reconcile_limit).await.map(Some)
                }
            }
        })
        .await;
        let metric_result = match attempt {
            Ok(Ok(processed)) => {
                if let Some(processed) = processed {
                    metrics.observe_batch(
                        WorkerLoop::OperationsAlertReconcile,
                        processed,
                        usize::from(runtime.config.reconcile_limit),
                    );
                }
                WorkerResult::Success
            }
            Ok(Err(error)) => {
                tracing::warn!(
                    error = %error,
                    operation = action.name(),
                    "operations alert worker cycle failed; retrying after poll delay"
                );
                WorkerResult::Error
            }
            Err(_) => {
                tracing::warn!(
                    operation = action.name(),
                    "operations alert worker cycle timed out; retrying after poll delay"
                );
                WorkerResult::Error
            }
        };
        metrics.observe_iteration(action.loop_kind(), metric_result, started.elapsed());

        let scheduled_wake = tokio::time::Instant::now() + runtime.config.poll_interval;
        if wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
            return;
        }
        metrics.observe_schedule_delay(
            action.loop_kind(),
            tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
        );
    }
}
