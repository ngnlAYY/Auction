//! Admin 装备录入 API 客户端。
//!
//! 浏览器只把用户当前填写的来源提交给后端；成功响应不返回装备 key 或来源正文。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::ApiResponse;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminIntakeResult {
    pub auction_id: String,
    pub auction_sequence: i64,
    pub auction_state_version: i64,
    pub preview_revision: i64,
    pub submission_id: String,
    pub lot_id: String,
    pub lot_number: i32,
    pub lot_state_version: i64,
    pub equipment_id: String,
    pub external_equipment_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_level: Option<i32>,
    pub start_price: i64,
    pub seller_player_id: String,
}

#[derive(Debug, Clone, Serialize)]
struct AdminIntakeRequest {
    command_id: Uuid,
    source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    equipment_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    equipment: Option<Value>,
    start_price: i64,
    player_id: String,
}

/// 提交 URL 或 canonical JSON 装备来源，并返回新建 lot 的安全投影。
pub(crate) async fn add_admin_intake(
    world_id: String,
    source_kind: String,
    equipment_url: String,
    equipment_json: String,
    start_price: i64,
    player_id: String,
) -> Result<AdminIntakeResult, String> {
    let equipment = if source_kind == "json" {
        Some(serde_json::from_str(&equipment_json).map_err(|error| error.to_string())?)
    } else {
        None
    };
    let request = Request::post(&format!("/api/v1/admin/worlds/{world_id}/auction-intake/items"))
        .header("x-csrf-token", &super::fetch_csrf_token().await?)
        .json(&AdminIntakeRequest {
            command_id: Uuid::new_v4(),
            source_kind,
            equipment_url: (equipment_url.is_empty()).then_some(equipment_url),
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
