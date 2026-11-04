//! 运行时开关只读客户端。

use super::{ApiResponse, invalid_response_error, response_error, send_get};
use auction_contracts::RuntimeSwitchListResponse;

pub(crate) async fn fetch_runtime_switches() -> Result<RuntimeSwitchListResponse, String> {
    let response = send_get("/api/v1/admin/runtime-switches").await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<RuntimeSwitchListResponse> =
        response.json().await.map_err(invalid_response_error)?;
    if payload.data.database_intent.len() != 3 || payload.data.admin_applied.len() != 2 {
        return Err("invalid_response".to_owned());
    }
    Ok(payload.data)
}
