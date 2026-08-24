//! Admin 装备录入 API 客户端。
//!
//! 浏览器只把用户当前填写的来源提交给后端；成功响应不返回装备 key 或来源正文。

pub(crate) use auction_contracts::AdminIntakeResponse as AdminIntakeResult;
use auction_contracts::{AdminCanonicalEquipmentRequest, AdminIntakeRequest};
use gloo_net::http::Request;
use uuid::Uuid;

use super::ApiResponse;

/// 提交 URL 或 canonical JSON 装备来源，并返回新建 lot 的安全投影。
///
/// `command_id` 由表单按同一输入保持不变，网络失败后的重试不会创建第二条 intake。
pub(crate) async fn add_admin_intake(
    command_id: Uuid,
    world_id: String,
    source_kind: String,
    equipment_url: String,
    equipment_json: String,
    start_price: i64,
    player_id: String,
) -> Result<AdminIntakeResult, String> {
    let equipment: Option<AdminCanonicalEquipmentRequest> = if source_kind == "json" {
        Some(serde_json::from_str(&equipment_json).map_err(|error| error.to_string())?)
    } else {
        None
    };
    let request = Request::post(&format!("/api/v1/admin/worlds/{world_id}/auction-intake/items"))
        .header("x-csrf-token", &super::fetch_csrf_token().await?)
        .json(&AdminIntakeRequest {
            command_id,
            source_kind: Some(source_kind),
            equipment_url: (!equipment_url.is_empty()).then_some(equipment_url),
            equipment,
            start_price,
            player_id,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminIntakeResult> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
