//! 告警台读投影、确认/静默只追加事实与导航待办计数。
//!
//! 读侧只读取 signal 水位和操作者事实，不重算阈值；写侧只追加一行
//! `operations_alert_acknowledgements` 并（静默时）推进 `next_notify_at`。确认与静默都不
//! 改写 kind/scope/source/count/age/active，也不跳过来源评估，因此下一次观察仍会恢复真实告警。

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::operations::{
    AdminPendingCounts, MAX_OPERATIONS_ALERT_WEBHOOK_VIEW, OperationsAlertAcknowledgeCommand,
    OperationsAlertCommandResult, OperationsAlertConsole, OperationsAlertConsolePage,
    OperationsAlertConsoleQuery, OperationsAlertError, OperationsAlertSilenceCommand,
    OperationsAlertWebhookEntry,
};
use chrono::{DateTime, Utc};

use super::DatabaseStore;

mod reads;
mod writes;

use reads::{list_postgres, list_webhooks_postgres, pending_counts_postgres};
use writes::{write_acknowledgement_postgres, write_silence_postgres};

impl OperationsAlertConsole for DatabaseStore {
    fn list_alerts(
        &self,
        query: OperationsAlertConsoleQuery,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, OperationsAlertConsolePage> {
        Box::pin(async move {
            query.validate().map_err(ApplicationError::from)?;
            if now.timestamp_nanos_opt().is_none() {
                return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
            }
            list_postgres(self.pool(), &query).await
        })
    }

    fn acknowledge_alert(
        &self,
        command: OperationsAlertAcknowledgeCommand,
    ) -> ApplicationFuture<'_, OperationsAlertCommandResult> {
        Box::pin(async move {
            command.validate().map_err(ApplicationError::from)?;
            write_acknowledgement_postgres(self.pool(), &command, None).await
        })
    }

    fn silence_alert(
        &self,
        command: OperationsAlertSilenceCommand,
    ) -> ApplicationFuture<'_, OperationsAlertCommandResult> {
        Box::pin(async move {
            command.validate().map_err(ApplicationError::from)?;
            write_silence_postgres(self.pool(), &command).await
        })
    }

    fn pending_counts(&self) -> ApplicationFuture<'_, AdminPendingCounts> {
        Box::pin(async move { pending_counts_postgres(self.pool()).await })
    }

    fn list_webhook_deliveries(
        &self,
        limit: usize,
    ) -> ApplicationFuture<'_, Vec<OperationsAlertWebhookEntry>> {
        Box::pin(async move {
            if limit == 0 || limit > MAX_OPERATIONS_ALERT_WEBHOOK_VIEW {
                return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
            }
            list_webhooks_postgres(self.pool(), limit).await
        })
    }
}
