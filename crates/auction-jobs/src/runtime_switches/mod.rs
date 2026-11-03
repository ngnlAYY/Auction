//! 三角色共用的周期唤醒；应用服务拥有刷新与到期规则。

use auction_application::{
    RuntimeSwitchRefreshService, RuntimeSwitchRole, RuntimeSwitchStore, RuntimeSwitches,
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
            if switches.status().role == RuntimeSwitchRole::Worker
                && !matches!(
                    tokio::time::timeout(Duration::from_secs(1), service.expire()).await,
                    Ok(Ok(()))
                )
            {
                tracing::warn!(
                    role = "worker",
                    "运行时开关到期收口未确认；后续按原命令重试，期限回落不受影响"
                );
            }
        }
    })
}

async fn refresh(service: &RuntimeSwitchRefreshService, switches: &RuntimeSwitches) {
    let previous = switches.readiness_policy();
    if matches!(tokio::time::timeout(Duration::from_secs(1), service.refresh()).await, Ok(Ok(()))) {
        let current = switches.readiness_policy();
        if previous != current {
            tracing::info!(
                role = switches.status().role.label(),
                audit_mode = auction_application::audit_label(current.audit_failure_mode),
                cache_enabled = current.cache_enabled,
                "运行时就绪开关已应用"
            );
        }
    } else {
        switches.refresh_failed();
        tracing::warn!(
            role = switches.status().role.label(),
            "运行时开关刷新失败；保留最后成功快照及原期限"
        );
    }
}
