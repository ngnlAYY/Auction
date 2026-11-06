//! Operations alert 三类周期循环的运行时编排；扫描、分发和对账彼此隔离，单轮失败不阻塞其他 owner。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use super::*;
use crate::worker::diagnostics;

#[derive(Debug, Clone, Copy)]
pub(super) enum OperationsAlertLoopAction {
    Scan,
    Dispatch,
    Reconcile,
}

impl OperationsAlertLoopAction {
    pub(super) const fn loop_kind(self) -> WorkerLoop {
        match self {
            Self::Scan => WorkerLoop::OperationsAlertScan,
            Self::Dispatch => WorkerLoop::OperationsAlertDispatch,
            Self::Reconcile => WorkerLoop::OperationsAlertReconcile,
        }
    }

    const fn diagnostic_phases(self) -> (&'static str, &'static str) {
        match self {
            Self::Scan => ("scan_sources", "scan_sources_timeout"),
            Self::Dispatch => ("dispatch_alert", "dispatch_alert_timeout"),
            Self::Reconcile => ("reconcile_delivery", "reconcile_delivery_timeout"),
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
    control: WorkerControlHandle,
    bulk_quota: Option<Arc<super::WorkerConnectionQuota>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    let diagnostic_metrics = metrics.clone();
    let diagnostic_loop = action.loop_kind();
    let body = async move {
        loop {
            if control.is_draining() {
                return;
            }
            let started = std::time::Instant::now();
            // W12.5：告警扫描/分发/对账属于批量轮次，受共享连接配额限制；许可只覆盖本轮查询。
            let bulk_permit = match &bulk_quota {
                Some(quota) => Some(quota.acquire().await),
                None => None,
            };
            let now = clock.now();
            let attempt = tokio::time::timeout(action.timeout(runtime.config), async {
                match action {
                    OperationsAlertLoopAction::Scan => runtime.scan(now).await.map(|()| None),
                    OperationsAlertLoopAction::Dispatch => {
                        runtime.dispatch(now).await.map(|()| None)
                    }
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
                    diagnostics::unpersisted_error(
                        "operations_alert",
                        action.diagnostic_phases().0,
                        error.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                    WorkerResult::Error
                }
                Err(error) => {
                    diagnostics::unpersisted_error(
                        "operations_alert",
                        action.diagnostic_phases().1,
                        diagnostics::timeout_kind(&error),
                        DiagnosticContext::default(),
                    );

                    WorkerResult::Error
                }
            };
            metrics.observe_iteration(action.loop_kind(), metric_result, started.elapsed());
            drop(bulk_permit);

            let wait = poll_delay(runtime.config.poll_interval, runtime.config.jitter);
            let scheduled_wake = tokio::time::Instant::now() + wait;
            if wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
                return;
            }
            metrics.observe_schedule_delay(
                action.loop_kind(),
                tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
            );
        }
    };
    diagnostics::scope(diagnostic_metrics, diagnostic_loop, body).await
}
