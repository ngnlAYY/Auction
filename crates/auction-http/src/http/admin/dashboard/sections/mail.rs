//! Admin dashboard 邮件轮询 section。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use auction_application::admin::{AdminMailDashboardSnapshot, AdminMailPort};
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::DashboardStatus;
use serde_json::json;

use super::super::super::timestamp;

/// 读取并校验邮件轮询快照；读取失败或形状不合法时返回 `Err`，由调用方降级为 unknown。
pub(crate) async fn load_mail_status(
    port: &dyn AdminMailPort,
) -> Result<AdminMailDashboardSnapshot, ()> {
    let snapshot = port.dashboard_snapshot().await.map_err(|diagnostic_cause| {
        diagnostics::warn(
            "admin_dashboard",
            "read_mail",
            DiagnosticError::diagnostic_kind(&diagnostic_cause),
            DiagnosticContext::default(),
        );
    })?;
    if !snapshot.validate() {
        return Err(());
    }
    Ok(snapshot)
}

pub(crate) fn map_mail_section(
    snapshot: &AdminMailDashboardSnapshot,
    configured_mailbox_keys: &[String],
) -> AdminDashboardSectionResponse {
    let scope_mismatch = !configured_mailbox_keys.is_empty()
        && (snapshot.pollers.len() != configured_mailbox_keys.len()
            || snapshot.pollers.iter().any(|poller| {
                !configured_mailbox_keys.iter().any(|key| key == &poller.mailbox_key)
            }));
    let error_code = if snapshot.pollers.is_empty() || scope_mismatch {
        Some("no_pollers")
    } else if snapshot.pollers.iter().any(|poller| poller.paused) {
        Some("polling_paused")
    } else if snapshot.pollers.iter().any(|poller| poller.consecutive_failures > 0) {
        Some("polling_failures")
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: if error_code.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(timestamp(snapshot.observed_at)),
        error_code: None,
        data: Some(json!({
            "observed_at": timestamp(snapshot.observed_at),
            "pollers": snapshot.pollers.iter().map(|poller| json!({
                "mailbox_key": poller.mailbox_key.clone(),
                "paused": poller.paused,
                "consecutive_failures": poller.consecutive_failures,
                "last_poll_at": timestamp(poller.last_poll_at),
                "next_poll_at": timestamp(poller.next_poll_at),
                "state_version": poller.state_version,
                "updated_at": timestamp(poller.updated_at),
            })).collect::<Vec<_>>(),
        })),
    }
}
