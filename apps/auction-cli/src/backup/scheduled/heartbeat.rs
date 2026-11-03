//! 终态之后的可选打卡；只向 stderr 输出状态和 HTTP 状态码，不回写备份回执。

use auction_config::AppConfig;
use auction_integrations::{BackupHeartbeatStatus, send_backup_heartbeat};

use super::model::{Outcome, RunReceipt};
use crate::app::progress;

pub(super) fn address(config: Option<&AppConfig>, outcome: Outcome) -> Option<&str> {
    config.and_then(|config| {
        if outcome == Outcome::Completed {
            config.backup_heartbeat_success_url()
        } else {
            config.backup_heartbeat_failure_url()
        }
    })
}

pub(super) async fn send(receipt: &RunReceipt, config: Option<&AppConfig>) {
    let result = send_backup_heartbeat(address(config, receipt.outcome)).await;
    let status = match result.status {
        BackupHeartbeatStatus::Sent => "sent",
        BackupHeartbeatStatus::Failed => "failed",
        BackupHeartbeatStatus::NotConfigured => "not_configured",
    };
    progress(format_args!(
        "backup run_id={} heartbeat={status} http_status={:?}",
        receipt.run_id, result.http_status
    ));
}
