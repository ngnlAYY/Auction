//! Operations alert Worker 的 PostgreSQL source、claim 与 delivery 装配。

use std::sync::Arc;

use anyhow::Context;
use auction_application::mail::MailDeliveryEnqueuer;
use auction_application::operations::{
    OperationsAlertRuntimeStatus, OperationsAlertSourceReader, OperationsAlertStore,
};
use auction_config::AppConfig;
use auction_integrations::OperationsAlertWebhookConfigError;
use auction_storage::DatabaseStore;
use auction_time::Timestamp;

pub(super) async fn configure_operations_alert(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
    delivery_enabled: bool,
) -> anyhow::Result<Option<Arc<auction_jobs::OperationsAlertRuntime>>> {
    let alert_config = &config.worker.operations_alert;
    let alert_store: Arc<dyn OperationsAlertStore> = store.clone();
    let scan_enabled = alert_config.scan_enabled();
    let notify_enabled = alert_config.notify_enabled();
    let webhook_enabled = alert_config.webhook_enabled();
    if !scan_enabled && !notify_enabled && !webhook_enabled {
        alert_store
            .publish_runtime_status(OperationsAlertRuntimeStatus {
                enabled: false,
                updated_at: Timestamp::now(),
            })
            .await
            .map_err(|error| anyhow::Error::msg(error.to_string()))
            .context("发布 operations alert 停用状态失败")?;
        return Ok(None);
    }
    if notify_enabled && !delivery_enabled {
        anyhow::bail!("启用 operations alert 主动通知时必须同时启用 MoogleMail delivery Worker");
    }
    let webhook: Option<Arc<dyn auction_application::operations::OperationsAlertWebhookSender>> =
        if webhook_enabled {
            let address = config
                .operations_alert_webhook_url()
                .context("启用 operations alert webhook 时必须配置 operations_alert_webhook_url")?;
            let sender = auction_integrations::OperationsAlertWebhook::new(address)
                .map_err(webhook_config_error)?;
            Some(Arc::new(sender))
        } else {
            None
        };
    let source: Arc<dyn OperationsAlertSourceReader> = store.clone();
    let (delivery, target) = if notify_enabled {
        let delivery: Arc<dyn MailDeliveryEnqueuer> = store;
        (Some(delivery), Some(alert_config.target()))
    } else {
        (None, None)
    };
    let claim_lease = std::time::Duration::from_secs(alert_config.claim_lease_seconds);
    let source_timeout = std::time::Duration::from_secs(alert_config.source_timeout_seconds);
    let dispatch_timeout = claim_lease
        .checked_sub(source_timeout)
        .context("operations alert 的投递超时必须小于领取租约")?;
    let policy = alert_config.policy();
    let runtime = auction_jobs::OperationsAlertRuntime::new(
        source,
        Arc::clone(&alert_store),
        delivery,
        policy,
        target,
        webhook.map(|sender| auction_jobs::OperationsAlertWebhookRuntime {
            sender,
            template: alert_config.webhook_template.clone(),
        }),
        auction_jobs::OperationsAlertRuntimeConfig {
            poll_interval: std::time::Duration::from_secs(alert_config.scan_interval_seconds),
            jitter: std::time::Duration::from_secs(alert_config.jitter_seconds),
            scan_timeout: claim_lease,
            dispatch_timeout,
            reconcile_timeout: claim_lease,
            reconcile_limit: u16::try_from(alert_config.max_batch_items)
                .context("operations alert 的对账批量上限超出 u16 范围")?,
        },
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
    alert_store
        .publish_runtime_status(OperationsAlertRuntimeStatus {
            enabled: true,
            updated_at: Timestamp::now(),
        })
        .await
        .map_err(|error| anyhow::Error::msg(error.to_string()))
        .context("发布 operations alert 启用状态失败")?;
    Ok(Some(Arc::new(runtime)))
}

/// 按变体给出可行动的启动错误：地址无效要改配置，客户端无法创建要查主机 TLS 根证书；
/// 两者都不回显地址原文。
fn webhook_config_error(error: OperationsAlertWebhookConfigError) -> anyhow::Error {
    let context = match error {
        OperationsAlertWebhookConfigError::InvalidAddress => {
            "operations alert webhook 地址无效：必须是不带用户信息与片段的绝对 HTTPS 地址"
        }
        OperationsAlertWebhookConfigError::ClientUnavailable => {
            "operations alert webhook 的受限 HTTPS 客户端无法创建：检查主机的系统 TLS 根证书"
        }
    };
    anyhow::Error::new(error).context(context)
}

#[cfg(test)]
mod tests {
    use auction_integrations::OperationsAlertWebhookConfigError;

    use super::webhook_config_error;

    #[test]
    fn webhook_config_errors_name_their_own_cause() {
        let invalid = format!(
            "{:#}",
            webhook_config_error(OperationsAlertWebhookConfigError::InvalidAddress)
        );
        assert!(invalid.contains("地址无效"), "{invalid}");
        assert!(!invalid.contains("客户端无法创建"), "{invalid}");

        let unavailable = format!(
            "{:#}",
            webhook_config_error(OperationsAlertWebhookConfigError::ClientUnavailable)
        );
        assert!(unavailable.contains("客户端无法创建"), "{unavailable}");
        assert!(!unavailable.contains("地址无效"), "{unavailable}");
    }
}
