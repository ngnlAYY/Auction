//! Admin dashboard realtime lag/failure section。

use auction_application::RealtimeAdminPort;
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::DashboardStatus;
use serde_json::json;

use super::super::super::timestamp;

pub(crate) async fn load_realtime_section(
    port: &dyn RealtimeAdminPort,
) -> Result<AdminDashboardSectionResponse, ()> {
    let snapshot = port.dashboard_snapshot().await.map_err(|_| ())?;
    if !snapshot.validate() {
        return Err(());
    }

    let error_code = if snapshot.failure.count > 0 {
        Some("unresolved_failure")
    } else if snapshot.total_lag > 0 {
        Some("consumer_lag")
    } else {
        None
    };
    Ok(AdminDashboardSectionResponse {
        status: if error_code.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(timestamp(snapshot.observed_at)),
        error_code: None,
        data: Some(json!({
            "observed_at": timestamp(snapshot.observed_at),
            "total_lag": snapshot.total_lag,
            "lags": snapshot.lags.into_iter().map(|entry| json!({
                "stream_key": entry.stream_key,
                "consumer_key": entry.consumer_key,
                "offset": entry.offset,
                "latest_sequence": entry.latest_sequence,
                "lag": entry.lag,
                "last_advanced_at": entry.last_advanced_at.map(timestamp),
            })).collect::<Vec<_>>(),
            "failure": {
                "count": snapshot.failure.count,
                "oldest_at": snapshot.failure.oldest_at.map(timestamp),
                "latest_code": snapshot.failure.latest_code,
            },
        })),
    })
}
