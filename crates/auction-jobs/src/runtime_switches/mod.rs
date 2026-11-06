//! 三角色共用的周期唤醒；应用服务拥有刷新与到期规则。

use crate::worker::diagnostics;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use auction_application::runtime_switches::{
    RuntimeSwitchError, RuntimeSwitchRefreshService, RuntimeSwitchRole, RuntimeSwitchStore,
    RuntimeSwitches,
};
use std::{sync::Arc, time::Duration};

/// 启动即读取，随后每五秒刷新；调用方用 TaskGuard 管理返回任务。
pub async fn start_runtime_switch_refresh(
    store: Arc<dyn RuntimeSwitchStore>,
    switches: RuntimeSwitches,
) -> tokio::task::JoinHandle<()> {
    let service = RuntimeSwitchRefreshService::new(store, switches.clone());
    refresh(&service, &switches).await;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(5),
            Duration::from_secs(5),
        );
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            refresh(&service, &switches).await;
            if switches.status().role == RuntimeSwitchRole::Worker {
                let attempt = tokio::time::timeout(Duration::from_secs(1), service.expire()).await;
                if !matches!(attempt, Ok(Ok(()))) {
                    record_failure(&attempt, "expire_override", "expire_override_timeout");
                }
            }
        }
    })
}

async fn refresh(service: &RuntimeSwitchRefreshService, switches: &RuntimeSwitches) {
    let previous = switches.readiness_policy();
    let attempt = tokio::time::timeout(Duration::from_secs(1), service.refresh()).await;
    if matches!(attempt, Ok(Ok(()))) {
        let current = switches.readiness_policy();
        if previous != current {
            tracing::info!(
                role = switches.status().role.label(),
                audit_mode =
                    auction_application::runtime_switches::audit_label(current.audit_failure_mode),
                cache_enabled = current.cache_enabled,
                "运行时就绪开关已应用"
            );
        }
    } else {
        switches.refresh_failed();
        record_failure(&attempt, "refresh_snapshot", "refresh_snapshot_timeout");
    }
}

// 只有观测使用此投影；失败计数已在 refresh_failed 中累加，日志过滤和去重不影响指标。
fn record_failure(
    attempt: &Result<Result<(), RuntimeSwitchError>, tokio::time::error::Elapsed>,
    phase: &'static str,
    timeout_phase: &'static str,
) {
    let (phase, safe_kind) = match attempt {
        Ok(Ok(())) => return,
        Ok(Err(cause)) => (phase, cause.diagnostic_kind()),
        Err(elapsed) => (timeout_phase, diagnostics::timeout_kind(elapsed)),
    };
    record_error(
        DiagnosticLevel::Warn,
        safe_kind,
        "runtime_switches",
        phase,
        DiagnosticContext::default(),
    );
}
