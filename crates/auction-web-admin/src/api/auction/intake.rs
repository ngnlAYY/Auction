//! Admin 装备录入 API 客户端。
//!
//! 浏览器只把用户当前填写的来源提交给后端；成功响应不返回装备 key 或来源正文。

pub(crate) use auction_contracts::AdminIntakeResponse as AdminIntakeResult;
use auction_contracts::{AdminCanonicalEquipmentRequest, AdminIntakeRequest};
use gloo_net::http::Request;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, RequiredOption, invalid_request_error,
    invalid_response_error, response_error, send_request, valid_trimmed_text, valid_uuid,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictIntakeResult {
    auction_id: String,
    auction_sequence: i64,
    auction_state_version: i64,
    equipment_category: String,
    equipment_id: String,
    equipment_level: RequiredOption<i32>,
    equipment_name: String,
    external_equipment_id: String,
    lot_id: String,
    lot_number: i32,
    lot_state_version: i64,
    preview_revision: i64,
    seller_player_id: String,
    start_price: i64,
    submission_id: String,
}

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
        Some(serde_json::from_str(&equipment_json).map_err(invalid_request_error)?)
    } else {
        None
    };
    let url = format!("/api/v1/admin/worlds/{world_id}/auction-intake/items");
    let csrf_token = super::fetch_csrf_token().await?;
    let command = AdminIntakeRequest {
        command_id: command_id.to_string(),
        source_kind: Some(source_kind),
        equipment_url: (!equipment_url.is_empty()).then_some(equipment_url),
        equipment,
        start_price,
        player_id,
    };
    let response = send_request(|abort_signal| {
        Request::post(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictIntakeResult> =
        response.json().await.map_err(invalid_response_error)?;
    let result = decode_intake_result(payload.data).map_err(invalid_response_error)?;
    if result.start_price != command.start_price
        || result.seller_player_id != command.player_id.trim()
    {
        return Err("invalid_response".to_owned());
    }
    Ok(result)
}

fn decode_intake_result(value: StrictIntakeResult) -> Result<AdminIntakeResult, &'static str> {
    if !valid_uuid(&value.auction_id)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.auction_sequence)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.auction_state_version)
        || !valid_trimmed_text(&value.equipment_category)
        || !valid_uuid(&value.equipment_id)
        || value.equipment_level.0.is_some_and(|level| level < 0)
        || !valid_trimmed_text(&value.equipment_name)
        || !valid_trimmed_text(&value.external_equipment_id)
        || !valid_uuid(&value.lot_id)
        || value.lot_number <= 0
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.lot_state_version)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.preview_revision)
        || !valid_trimmed_text(&value.seller_player_id)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.start_price)
        || !valid_uuid(&value.submission_id)
    {
        return Err("intake_result");
    }
    Ok(AdminIntakeResult {
        auction_id: Uuid::parse_str(&value.auction_id).map_err(|_| "intake_auction_id")?,
        auction_sequence: value.auction_sequence,
        auction_state_version: value.auction_state_version,
        preview_revision: value.preview_revision,
        submission_id: Uuid::parse_str(&value.submission_id).map_err(|_| "intake_submission_id")?,
        lot_id: Uuid::parse_str(&value.lot_id).map_err(|_| "intake_lot_id")?,
        lot_number: value.lot_number,
        lot_state_version: value.lot_state_version,
        equipment_id: Uuid::parse_str(&value.equipment_id).map_err(|_| "intake_equipment_id")?,
        external_equipment_id: value.external_equipment_id,
        equipment_name: value.equipment_name,
        equipment_category: value.equipment_category,
        equipment_level: value.equipment_level.0,
        start_price: value.start_price,
        seller_player_id: value.seller_player_id,
    })
}
