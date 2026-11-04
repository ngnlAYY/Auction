//! 运行时开关只读客户端。

use super::{ApiResponse, invalid_response_error, response_error, send_get};
use auction_contracts::RuntimeSwitchListResponse;

const KNOWN_SWITCHES: [&str; 5] = [
    "readiness_audit_failure_mode",
    "readiness_cache_enabled",
    "metrics_sli_snapshot_enabled",
    "mooglemail_poll_isolation_enabled",
    "metrics_reject_forwarded_requests",
];

pub(crate) async fn fetch_runtime_switches() -> Result<RuntimeSwitchListResponse, String> {
    let response = send_get("/api/v1/admin/runtime-switches").await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<RuntimeSwitchListResponse> =
        response.json().await.map_err(invalid_response_error)?;
    let database_names =
        payload.data.database_intent.iter().map(|item| item.name.as_str()).collect::<Vec<_>>();
    let applied_names =
        payload.data.admin_applied.iter().map(|item| item.name.as_str()).collect::<Vec<_>>();
    if !closed_subset(&database_names)
        || !closed_subset(&applied_names)
        || !applied_names.iter().all(|name| database_names.contains(name))
    {
        return Err("invalid_response".to_owned());
    }
    Ok(payload.data)
}

fn closed_subset(names: &[&str]) -> bool {
    names.iter().all(|name| KNOWN_SWITCHES.contains(name))
        && names.iter().enumerate().all(|(index, name)| !names[..index].contains(name))
}
