//! Operations alert Worker 的 PostgreSQL source、claim 与 delivery 装配。

#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::*;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(super) async fn configure_operations_alert(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
    delivery_enabled: bool,
) -> anyhow::Result<Option<Arc<auction_jobs::OperationsAlertRuntime>>> {
    let alert_config = &config.worker.operations_alert;
    let alert_store: Arc<dyn OperationsAlertStore> = store.clone();
    if !alert_config.enabled {
        alert_store
            .publish_runtime_status(OperationsAlertRuntimeStatus {
                enabled: false,
                updated_at: Utc::now(),
            })
            .await
            .map_err(|error| anyhow::Error::msg(error.to_string()))
            .context("publish disabled operations alert runtime status")?;
        return Ok(None);
    }
    if store.backend() != DatabaseBackend::Postgres {
        anyhow::bail!("operations alert 仅支持 PostgreSQL 完整档");
    }
    if !delivery_enabled {
        anyhow::bail!("启用 operations alert 时必须同时启用 MoogleMail delivery Worker");
    }
    let source: Arc<dyn OperationsAlertSourceReader> = store.clone();
    let delivery: Arc<dyn MailDeliveryEnqueuer> = store;
    let claim_lease = std::time::Duration::from_secs(alert_config.claim_lease_seconds);
    let source_timeout = std::time::Duration::from_secs(alert_config.source_timeout_seconds);
    let dispatch_timeout = claim_lease
        .checked_sub(source_timeout)
        .context("operations alert dispatch timeout must remain inside claim lease")?;
    let runtime = auction_jobs::OperationsAlertRuntime::new(
        source,
        Arc::clone(&alert_store),
        delivery,
        alert_config.policy(),
        alert_config.target(),
        auction_jobs::OperationsAlertRuntimeConfig {
            poll_interval: std::time::Duration::from_secs(alert_config.scan_interval_seconds),
            jitter: std::time::Duration::from_secs(alert_config.jitter_seconds),
            scan_timeout: claim_lease,
            dispatch_timeout,
            reconcile_timeout: claim_lease,
            reconcile_limit: u16::try_from(alert_config.max_batch_items)
                .context("operations alert reconcile limit exceeds u16")?,
        },
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
    alert_store
        .publish_runtime_status(OperationsAlertRuntimeStatus {
            enabled: true,
            updated_at: Utc::now(),
        })
        .await
        .map_err(|error| anyhow::Error::msg(error.to_string()))
        .context("publish enabled operations alert runtime status")?;
    Ok(Some(Arc::new(runtime)))
}
