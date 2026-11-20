//! Admin 装备录入 API 客户端。
//! 浏览器只把用户当前填写的来源提交给后端；成功响应不返回装备 key 或来源正文。

pub(crate) use auction_contracts::AdminIntakeResponse as AdminIntakeResult;
use auction_contracts::{AdminCanonicalEquipmentRequest, AdminIntakeRequest, ApiResponse};
use gloo_net::http::Method;
use uuid::Uuid;

use super::AdminWriteFailure;
use super::transport::WriteRequest;
use super::{MAX_JSON_SAFE_INTEGER, invalid_request_error, valid_trimmed_text};

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
) -> Result<AdminIntakeResult, AdminWriteFailure> {
    let equipment: Option<AdminCanonicalEquipmentRequest> = if source_kind == "json" {
        Some(
            serde_json::from_str(&equipment_json)
                .map_err(|error| AdminWriteFailure::not_sent(invalid_request_error(error)))?,
        )
    } else {
        None
    };
    let url = format!("/api/v1/admin/worlds/{world_id}/auction-intake/items");
    let command = AdminIntakeRequest {
        command_id: command_id.to_string(),
        equipment_url: (source_kind == "url" && !equipment_url.is_empty()).then_some(equipment_url),
        source_kind: Some(source_kind),
        equipment,
        start_price,
        player_id,
    };
    let response = WriteRequest::general(&url, Method::POST).send_json(&command).await?;
    let payload: ApiResponse<AdminIntakeResult> =
        response.json().await.map_err(AdminWriteFailure::invalid_response)?;
    decode_intake_receipt(payload, &command)
}

fn decode_intake_receipt(
    payload: ApiResponse<AdminIntakeResult>,
    command: &AdminIntakeRequest,
) -> Result<AdminIntakeResult, AdminWriteFailure> {
    let result = decode_intake_result(payload.data).map_err(AdminWriteFailure::invalid_response)?;
    if result.start_price != command.start_price
        || result.seller_player_id != command.player_id.trim()
    {
        return Err(AdminWriteFailure::outcome_unknown("invalid_response"));
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

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use uuid::Uuid;

    use super::{
        AdminIntakeRequest, AdminIntakeResult, ApiResponse, add_admin_intake,
        decode_intake_receipt, decode_intake_result,
    };
    use crate::api::AdminWriteFailure;
    use crate::api::contract_fixtures::{admin_fixture, decode};

    #[test]
    fn intake_receipt_delivers_only_the_matching_seller_and_price() {
        let command = AdminIntakeRequest {
            command_id: "00000000-0000-4000-8000-000000000002".to_owned(),
            equipment_url: Some("https://hentaiverse.org/equip/123/0123456789".to_owned()),
            source_kind: Some("url".to_owned()),
            equipment: None,
            start_price: 1000,
            player_id: "Player".to_owned(),
        };
        let payload: ApiResponse<AdminIntakeResult> =
            decode(admin_fixture!("lot-intake-command"), "/data", "equipment_level");
        assert_eq!(decode_intake_receipt(payload.clone(), &command).unwrap(), payload.data);

        let mut different_seller = payload.clone();
        different_seller.data.seller_player_id = "OtherPlayer".to_owned();
        let mut different_price = payload;
        different_price.data.start_price = 1001;
        for (field, receipt) in
            [("seller_player_id", different_seller), ("start_price", different_price)]
        {
            assert_eq!(
                decode_intake_receipt(receipt, &command).unwrap_err(),
                AdminWriteFailure::outcome_unknown("invalid_response"),
                "{field}"
            );
        }
    }

    #[test]
    fn intake_command_contract_fixture() {
        let payload: ApiResponse<AdminIntakeResult> =
            decode(admin_fixture!("lot-intake-command"), "/data", "equipment_level");
        assert_eq!(payload.code, "created");
        let result = decode_intake_result(payload.data).expect("录入拍品回执");
        assert_eq!(result.lot_number, 1);
        assert_eq!(result.start_price, 1000);
        assert_eq!(result.seller_player_id, "Player");
        assert!(result.equipment_level.is_none());
    }

    #[test]
    fn invalid_intake_equipment_is_rejected_before_review_or_write() {
        let mut command = pin!(add_admin_intake(
            Uuid::from_u128(0x00000000_0000_4000_8000_000000000002),
            "main".to_owned(),
            "json".to_owned(),
            String::new(),
            r#"{"prefix":"旧","prefix":""}"#.to_owned(),
            1000,
            "Player".to_owned(),
        ));
        let Poll::Ready(Err(error)) =
            command.as_mut().poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("无效装备应在复核会话前返回");
        };
        assert_eq!(error, AdminWriteFailure::not_sent("invalid_request"));
    }
}
