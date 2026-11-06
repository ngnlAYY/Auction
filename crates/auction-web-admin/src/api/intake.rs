//! Admin 装备录入 API 客户端。
//! 浏览器只把用户当前填写的来源提交给后端；成功响应不返回装备 key 或来源正文。

pub(crate) use auction_contracts::AdminIntakeResponse as AdminIntakeResult;
use auction_contracts::{AdminCanonicalEquipmentRequest, AdminIntakeRequest};
use gloo_net::http::Request;
use uuid::Uuid;

use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, invalid_request_error, invalid_response_error,
    response_error, send_request, valid_trimmed_text,
};

// 结构闭集由 contracts 的 `AdminIntakeResponse`（`deny_unknown_fields`，`equipment_level`
// 保持旧 decoder 的必填 nullable 语义）负责；标识字段直接以 `Uuid` 解码（接受 RFC 4122
// 全版本），其余值域谓词保留在下方校验函数。

/// 提交 URL 或 canonical JSON 装备来源，并返回新建 lot 的安全投影。
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
        equipment_url: (source_kind == "url" && !equipment_url.is_empty()).then_some(equipment_url),
        source_kind: Some(source_kind),
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
    let payload: ApiResponse<AdminIntakeResult> =
        response.json().await.map_err(invalid_response_error)?;
    let result = decode_intake_result(payload.data).map_err(invalid_response_error)?;
    if result.start_price != command.start_price
        || result.seller_player_id != command.player_id.trim()
    {
        return Err("invalid_response".to_owned());
    }
    Ok(result)
}

fn decode_intake_result(value: AdminIntakeResult) -> Result<AdminIntakeResult, &'static str> {
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.auction_sequence)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.auction_state_version)
        || !valid_trimmed_text(&value.equipment_category)
        || value.equipment_level.is_some_and(|level| level < 0)
        || !valid_trimmed_text(&value.equipment_name)
        || !valid_trimmed_text(&value.external_equipment_id)
        || value.lot_number <= 0
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.lot_state_version)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.preview_revision)
        || !valid_trimmed_text(&value.seller_player_id)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.start_price)
    {
        return Err("intake_result");
    }
    Ok(value)
}
