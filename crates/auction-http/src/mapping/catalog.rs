//! Public 拍卖、标的、装备和游标契约映射。

use auction_application::auction::{
    CurrentAuctionSnapshot, PublicAuctionCursor, PublicAuctionProjection, PublicAuctionSummary,
    PublicBidLogCursor, PublicEquipment, PublicFinalRestriction, PublicLiveLotSummary,
    PublicLotDetail, PublicLotPage, PublicPreviewLotDetail, PublicPreviewLotSummary,
    PublicPriceHistoryCursor, format_public_time_wire,
};
use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::public::PublicEquipmentLinkTarget;
use auction_contracts::AuctionSummaryV2Response;
use auction_contracts::{
    AuctionSummaryResponse, CurrentAuctionItemResponse, PublicEquipmentResponse,
    PublicEquipmentStatResponse, PublicFinalRestrictionResponse, PublicLiveLotResponse,
    PublicLotDetailItemResponse, PublicLotEnvelopeResponse, PublicLotListItemsResponse,
    PublicPreviewEquipmentResponse, PublicPreviewLotDetailResponse, PublicPreviewLotResponse,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, SecondsFormat, Utc};
use uuid::Uuid;

use crate::diagnostics;

pub(crate) fn map_auction_summary(summary: PublicAuctionSummary) -> AuctionSummaryResponse {
    AuctionSummaryResponse {
        id: summary.id.to_string(),
        world_id: summary.world_id,
        sequence: summary.sequence,
        status: summary.status,
        scheduled_open_at: public_timestamp(summary.scheduled_open_at),
        opened_at: summary.opened_at.map(public_timestamp),
        final_phase_at: summary.final_phase_at.map(public_timestamp),
        effective_end_at: summary.effective_end_at.map(public_timestamp),
        settled_at: summary.settled_at.map(public_timestamp),
        lot_count: summary.lot_count,
        sold_count: summary.sold_count,
        unsold_count: summary.unsold_count,
        sold_amount: summary.sold_amount,
        state_version: summary.state_version,
    }
}

/// 论坛地址只进入 v2 DTO；v1 映射始终序列化原字段集合。
pub(crate) fn map_auction_summary_v2(
    mut summary: PublicAuctionSummary,
) -> AuctionSummaryV2Response {
    let forum_thread_url =
        summary.forum_link.take().map(auction_application::forum::PublicForumLink::into_url);
    AuctionSummaryV2Response { auction: map_auction_summary(summary), forum_thread_url }
}

pub(crate) fn map_current_auction(auction: CurrentAuctionSnapshot) -> CurrentAuctionItemResponse {
    CurrentAuctionItemResponse {
        id: auction.id.to_string(),
        world: auction.world_id,
        number: auction.sequence,
        status: auction.status,
        opens_at: public_timestamp(auction.opens_at),
        scheduled_ends_at: auction.scheduled_ends_at.map(public_timestamp),
        presentation_mode: auction.presentation_mode,
        bidding_open: auction.bidding_open,
        preview_revision: auction.preview_revision,
    }
}

pub(crate) fn map_lot_page(
    page: PublicLotPage,
    page_number: i64,
    game_equipment_urls: Vec<String>,
) -> Option<PublicLotEnvelopeResponse> {
    let projection = page.projection;
    let expected_links =
        if projection.projection_kind == "preview" { page.preview.len() } else { page.live.len() };
    if game_equipment_urls.len() != expected_links {
        return None;
    }
    let lots = if projection.projection_kind == "preview" {
        PublicLotListItemsResponse::Preview(
            page.preview
                .into_iter()
                .zip(game_equipment_urls)
                .map(|(lot, game_equipment_url)| {
                    let mut mapped = map_preview_lot(lot);
                    mapped.game_equipment_url = game_equipment_url;
                    mapped
                })
                .collect(),
        )
    } else {
        PublicLotListItemsResponse::Live(
            page.live
                .into_iter()
                .zip(game_equipment_urls)
                .map(|(lot, game_equipment_url)| {
                    let mut mapped = map_live_lot(lot, None);
                    mapped.equipment.game_equipment_url = game_equipment_url;
                    mapped
                })
                .collect(),
        )
    };
    let response = map_lot_envelope(projection);
    Some(PublicLotEnvelopeResponse {
        lots: Some(lots),
        total: Some(page.total),
        page: Some(page_number),
        page_size: Some(page.limit),
        category_counts: Some(page.category_counts),
        ..response
    })
}

/// 接管已校验投影中的旧链接；调用方只能在解析器成功后映射替换链接，不能再直接返回原投影。
pub(crate) fn lot_page_equipment_targets(
    page: &mut PublicLotPage,
) -> Result<Vec<PublicEquipmentLinkTarget>, ()> {
    let targets = if page.projection.projection_kind == "preview" {
        page.preview
            .iter_mut()
            .map(|lot| PublicEquipmentLinkTarget {
                lot_id: lot.id,
                equipment_id: lot.equipment_id,
                world_id: page.projection.world_id.clone(),
                projected_game_equipment_url: std::mem::take(&mut lot.game_equipment_url),
            })
            .collect::<Vec<_>>()
    } else {
        page.live
            .iter_mut()
            .map(|lot| PublicEquipmentLinkTarget {
                lot_id: lot.id,
                equipment_id: lot.equipment_id,
                world_id: page.projection.world_id.clone(),
                projected_game_equipment_url: std::mem::take(&mut lot.equipment.game_equipment_url),
            })
            .collect::<Vec<_>>()
    };
    if targets.iter().any(|target| target.validate().is_err()) {
        return Err(());
    }
    Ok(targets)
}

/// 详情与列表使用同一链接所有权转移，保留完整投影校验在提取之前执行。
pub(crate) fn lot_detail_equipment_target(
    detail: &mut PublicLotDetail,
    lot_id: Uuid,
) -> Result<PublicEquipmentLinkTarget, ()> {
    let projected_game_equipment_url = detail
        .preview
        .as_mut()
        .map(|preview| std::mem::take(&mut preview.equipment.game_equipment_url))
        .or_else(|| {
            detail
                .live
                .as_mut()
                .map(|live| std::mem::take(&mut live.summary.equipment.game_equipment_url))
        })
        .ok_or(())?;
    let target = PublicEquipmentLinkTarget {
        lot_id,
        equipment_id: detail.equipment_id,
        world_id: detail.projection.world_id.clone(),
        projected_game_equipment_url,
    };
    target.validate().map_err(|diagnostic_cause| {
        diagnostics::error(
            "public_catalog",
            "validate_equipment_target",
            diagnostics::invalid_state(&diagnostic_cause),
            DiagnosticContext::default(),
        );
    })?;
    Ok(target)
}

pub(crate) fn map_lot_detail(
    detail: PublicLotDetail,
    game_equipment_url: Option<String>,
) -> PublicLotEnvelopeResponse {
    let response = map_lot_envelope(detail.projection);
    let lot = match (detail.preview, detail.live) {
        (Some(preview), None) => {
            let mut mapped = map_preview_detail(preview);
            let game_equipment_url = game_equipment_url.unwrap_or_default();
            mapped.summary.game_equipment_url.clone_from(&game_equipment_url);
            mapped.equipment.game_equipment_url = game_equipment_url;
            PublicLotDetailItemResponse::Preview(mapped)
        }
        (None, Some(mut live)) => {
            // 正式详情属性只持有一份；映射前移交给 wire，而不是复制整棵 JSON。
            let attributes = std::mem::take(&mut live.summary.equipment.attributes);
            let mut mapped = map_live_lot(live.summary, Some(attributes));
            mapped.equipment.game_equipment_url = game_equipment_url.unwrap_or_default();
            PublicLotDetailItemResponse::Live(mapped)
        }
        (None, None) | (Some(_), Some(_)) => {
            // 存储端口的 projection 与分支不一致时由边界校验拦截；这里没有可安全序列化的 fallback。
            return PublicLotEnvelopeResponse { lot: None, ..response };
        }
    };
    PublicLotEnvelopeResponse { lot: Some(lot), ..response }
}

pub(crate) fn map_lot_envelope(projection: PublicAuctionProjection) -> PublicLotEnvelopeResponse {
    PublicLotEnvelopeResponse {
        auction_id: projection.auction_id.to_string(),
        auction_status: projection.auction_status,
        projection_kind: projection.projection_kind,
        presentation_mode: projection.presentation_mode,
        bidding_open: projection.bidding_open,
        opens_at: public_timestamp(projection.opens_at),
        preview_revision: projection.preview_revision,
        final_restriction: projection.final_restriction.map(map_final_restriction),
        lots: None,
        lot: None,
        total: None,
        page: None,
        page_size: None,
        category_counts: None,
    }
}

pub(crate) fn map_final_restriction(
    restriction: PublicFinalRestriction,
) -> PublicFinalRestrictionResponse {
    PublicFinalRestrictionResponse {
        new_slow_proxy_allowed: restriction.new_slow_proxy_allowed,
        existing_slow_proxy_converted: restriction.existing_slow_proxy_converted,
    }
}

pub(crate) fn map_live_lot(
    lot: PublicLiveLotSummary,
    attributes: Option<serde_json::Value>,
) -> PublicLiveLotResponse {
    PublicLiveLotResponse {
        id: lot.id.to_string(),
        auction_id: lot.auction_id.to_string(),
        lot_number: lot.lot_number,
        status: lot.status,
        start_price: lot.start_price,
        current_price: lot.current_price,
        next_bid: lot.next_bid,
        has_leader: lot.has_leader,
        leader_name: lot.leader_name,
        seller_name: lot.seller_name,
        effective_end_at: lot.effective_end_at.map(public_timestamp),
        state_version: lot.state_version,
        equipment: map_equipment(lot.equipment, attributes),
    }
}

pub(crate) fn map_preview_lot(lot: PublicPreviewLotSummary) -> PublicPreviewLotResponse {
    PublicPreviewLotResponse {
        id: lot.id.to_string(),
        auction_id: lot.auction_id.to_string(),
        code: lot.code,
        status: lot.status,
        seller_name: lot.seller_name,
        start_price: lot.start_price,
        equipment_name: lot.equipment_name,
        equipment_group: lot.equipment_group.as_str().to_owned(),
        equipment_category: (!lot.equipment_category.is_empty()).then_some(lot.equipment_category),
        equipment_level: lot.equipment_level,
        primary_attribute_initials: lot.primary_attribute_initials,
        avg_roll: lot.avg_roll,
        game_equipment_url: lot.game_equipment_url,
    }
}

pub(crate) fn map_preview_detail(detail: PublicPreviewLotDetail) -> PublicPreviewLotDetailResponse {
    PublicPreviewLotDetailResponse {
        summary: map_preview_lot(detail.summary),
        equipment: map_preview_equipment(detail.equipment),
    }
}

fn map_preview_equipment(equipment: PublicEquipment) -> PublicPreviewEquipmentResponse {
    let primary_attribute_initials = equipment.primary_attribute_initials();
    let stats = equipment
        .stats
        .into_iter()
        .map(|(name, stat)| {
            (
                name,
                PublicEquipmentStatResponse { value: stat.value, base: stat.base, roll: stat.roll },
            )
        })
        .collect();
    PublicPreviewEquipmentResponse {
        name: equipment.name,
        equipment_group: equipment.equipment_group.as_str().to_owned(),
        category: (!equipment.category.is_empty()).then_some(equipment.category),
        level: equipment.level,
        primary_attribute_initials,
        info_summary: equipment.info_summary,
        stats,
        avg_roll: equipment.avg_roll,
        game_equipment_url: equipment.game_equipment_url,
    }
}

pub(crate) fn map_equipment(
    equipment: PublicEquipment,
    attributes: Option<serde_json::Value>,
) -> PublicEquipmentResponse {
    let primary_attribute_initials = equipment.primary_attribute_initials();
    let stats = equipment
        .stats
        .into_iter()
        .map(|(name, stat)| {
            (
                name,
                PublicEquipmentStatResponse { value: stat.value, base: stat.base, roll: stat.roll },
            )
        })
        .collect();
    PublicEquipmentResponse {
        name: equipment.name,
        category: equipment.category,
        equipment_group: equipment.equipment_group.as_str().to_owned(),
        level: equipment.level,
        primary_attribute_initials,
        info_summary: equipment.info_summary,
        stats,
        avg_roll: equipment.avg_roll,
        game_equipment_url: equipment.game_equipment_url,
        attributes,
    }
}

pub(crate) fn valid_lot_detail(detail: &PublicLotDetail, lot_id: Uuid) -> bool {
    detail.validate(lot_id).is_ok()
}

pub(crate) fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Public 沿用旧 Go `time.Time` 的 `RFC3339Nano` 输出；Admin 继续使用上方既有微秒契约。
pub(crate) fn public_timestamp(value: DateTime<Utc>) -> String {
    format_public_time_wire(value)
}

pub(crate) fn encode_public_cursor(cursor: &PublicAuctionCursor) -> String {
    match cursor {
        PublicAuctionCursor::Start => "start".to_owned(),
        PublicAuctionCursor::Boundary { sequence, id } => {
            let mut payload = [0_u8; 26];
            payload[0] = 1;
            payload[1] = 3;
            payload[2..10].copy_from_slice(&sequence.to_be_bytes());
            payload[10..].copy_from_slice(id.as_bytes());
            URL_SAFE_NO_PAD.encode(payload)
        }
    }
}

pub(crate) fn decode_public_cursor(raw: &str) -> Result<PublicAuctionCursor, ()> {
    if raw == "start" {
        return Ok(PublicAuctionCursor::Start);
    }
    // 固定 26 字节只接受 35 个无填充字符；严格引擎已拒绝非规范尾位，无需重编码分配。
    if raw.len() != 35 {
        return Err(());
    }
    let mut decoded = [0_u8; 26];
    let decoded_len = URL_SAFE_NO_PAD.decode_slice(raw, &mut decoded).map_err(|_| ())?;
    if decoded_len != decoded.len() || decoded[0] != 1 || decoded[1] != 3 {
        return Err(());
    }
    let sequence = i64::from_be_bytes(decoded[2..10].try_into().map_err(|_| ())?);
    let id = Uuid::from_slice(&decoded[10..]).map_err(|_| ())?;
    if sequence <= 0 || id.is_nil() {
        return Err(());
    }
    Ok(PublicAuctionCursor::Boundary { sequence, id })
}

pub(crate) fn encode_public_bid_log_cursor(cursor: &PublicBidLogCursor) -> String {
    match cursor {
        PublicBidLogCursor::Start => "start".to_owned(),
        PublicBidLogCursor::Boundary {
            created_at,
            lot_id,
            accepted_state_version,
            fact_index,
            id,
        } => {
            let mut payload = [0_u8; 54];
            payload[0] = 1;
            payload[1] = 4;
            payload[2..10].copy_from_slice(&created_at.timestamp_micros().to_be_bytes());
            payload[10..26].copy_from_slice(lot_id.as_bytes());
            payload[26..34].copy_from_slice(&accepted_state_version.to_be_bytes());
            payload[34..38].copy_from_slice(&fact_index.to_be_bytes());
            payload[38..54].copy_from_slice(id.as_bytes());
            URL_SAFE_NO_PAD.encode(payload)
        }
    }
}

pub(crate) fn decode_public_bid_log_cursor(raw: &str) -> Result<PublicBidLogCursor, ()> {
    if raw == "start" {
        return Ok(PublicBidLogCursor::Start);
    }
    // v1/type4 固定 54 字节；与目录游标相同，直接解码到栈并保留严格字母表/尾位校验。
    if raw.len() != 72 {
        return Err(());
    }
    let mut decoded = [0_u8; 54];
    let decoded_len = URL_SAFE_NO_PAD.decode_slice(raw, &mut decoded).map_err(|_| ())?;
    if decoded_len != decoded.len() || decoded[0] != 1 || decoded[1] != 4 {
        return Err(());
    }
    let timestamp_micros = i64::from_be_bytes(decoded[2..10].try_into().map_err(|_| ())?);
    let created_at = DateTime::from_timestamp_micros(timestamp_micros).ok_or(())?;
    let lot_id = Uuid::from_slice(&decoded[10..26]).map_err(|_| ())?;
    let accepted_state_version = i64::from_be_bytes(decoded[26..34].try_into().map_err(|_| ())?);
    let fact_index = i32::from_be_bytes(decoded[34..38].try_into().map_err(|_| ())?);
    let id = Uuid::from_slice(&decoded[38..54]).map_err(|_| ())?;
    if timestamp_micros == 0
        || lot_id.is_nil()
        || accepted_state_version <= 0
        || fact_index <= 0
        || id.is_nil()
    {
        return Err(());
    }
    Ok(PublicBidLogCursor::Boundary { created_at, lot_id, accepted_state_version, fact_index, id })
}

pub(crate) fn encode_public_price_history_cursor(cursor: &PublicPriceHistoryCursor) -> String {
    // v1/type5 的词法只表达 closed_desc，其他排序在 HTTP 与应用请求校验中拒绝 cursor。
    match cursor {
        PublicPriceHistoryCursor::Start => "start".to_owned(),
        PublicPriceHistoryCursor::Boundary { closed_at, auction_sequence, lot_number, lot_id } => {
            let mut payload = [0_u8; 38];
            payload[0] = 1;
            payload[1] = 5;
            payload[2..10].copy_from_slice(&closed_at.timestamp_micros().to_be_bytes());
            payload[10..18].copy_from_slice(&auction_sequence.to_be_bytes());
            payload[18..22].copy_from_slice(&(*lot_number as i32).to_be_bytes());
            payload[22..38].copy_from_slice(lot_id.as_bytes());
            URL_SAFE_NO_PAD.encode(payload)
        }
    }
}

pub(crate) fn decode_public_price_history_cursor(
    raw: &str,
) -> Result<PublicPriceHistoryCursor, ()> {
    if raw == "start" {
        return Ok(PublicPriceHistoryCursor::Start);
    }
    // v1/type5 固定 38 字节，对应 51 个无填充字符；不为无效长度或规范性复核分配堆内存。
    if raw.len() != 51 {
        return Err(());
    }
    let mut decoded = [0_u8; 38];
    let decoded_len = URL_SAFE_NO_PAD.decode_slice(raw, &mut decoded).map_err(|_| ())?;
    if decoded_len != decoded.len() || decoded[0] != 1 || decoded[1] != 5 {
        return Err(());
    }
    let timestamp_micros = i64::from_be_bytes(decoded[2..10].try_into().map_err(|_| ())?);
    let closed_at = DateTime::from_timestamp_micros(timestamp_micros).ok_or(())?;
    let auction_sequence = i64::from_be_bytes(decoded[10..18].try_into().map_err(|_| ())?);
    let lot_number = i64::from(i32::from_be_bytes(decoded[18..22].try_into().map_err(|_| ())?));
    let lot_id = Uuid::from_slice(&decoded[22..38]).map_err(|_| ())?;
    if timestamp_micros == 0 || auction_sequence <= 0 || lot_number <= 0 || lot_id.is_nil() {
        return Err(());
    }
    Ok(PublicPriceHistoryCursor::Boundary { closed_at, auction_sequence, lot_number, lot_id })
}
