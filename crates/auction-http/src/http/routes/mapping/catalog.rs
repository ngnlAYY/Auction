//! Public 拍卖、标的、装备和游标契约映射。

use super::super::*;

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
    let mut game_equipment_urls = game_equipment_urls.into_iter();
    let lots = if projection.projection_kind == "preview" {
        PublicLotListItemsResponse::Preview(
            page.preview
                .into_iter()
                .map(|lot| {
                    let mut mapped = map_preview_lot(lot);
                    mapped.game_equipment_url = game_equipment_urls.next().unwrap_or_default();
                    mapped
                })
                .collect(),
        )
    } else {
        PublicLotListItemsResponse::Live(
            page.live
                .into_iter()
                .map(|lot| {
                    let mut mapped = map_live_lot(lot, None);
                    mapped.equipment.game_equipment_url =
                        game_equipment_urls.next().unwrap_or_default();
                    mapped
                })
                .collect(),
        )
    };
    if game_equipment_urls.next().is_some() {
        return None;
    }
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

pub(crate) fn lot_page_equipment_targets(
    page: &PublicLotPage,
) -> Result<Vec<PublicEquipmentLinkTarget>, ()> {
    let targets = if page.projection.projection_kind == "preview" {
        page.preview
            .iter()
            .map(|lot| PublicEquipmentLinkTarget {
                lot_id: lot.id,
                equipment_id: lot.equipment_id,
                world_id: page.projection.world_id.clone(),
                projected_game_equipment_url: lot.game_equipment_url.clone(),
            })
            .collect::<Vec<_>>()
    } else {
        page.live
            .iter()
            .map(|lot| PublicEquipmentLinkTarget {
                lot_id: lot.id,
                equipment_id: lot.equipment_id,
                world_id: page.projection.world_id.clone(),
                projected_game_equipment_url: lot.equipment.game_equipment_url.clone(),
            })
            .collect::<Vec<_>>()
    };
    if targets.iter().any(|target| target.validate().is_err()) {
        return Err(());
    }
    Ok(targets)
}

pub(crate) fn lot_detail_equipment_target(
    detail: &PublicLotDetail,
    lot_id: Uuid,
) -> Result<PublicEquipmentLinkTarget, ()> {
    let projected_game_equipment_url = detail
        .preview
        .as_ref()
        .map(|preview| preview.equipment.game_equipment_url.clone())
        .or_else(|| {
            detail.live.as_ref().map(|live| live.summary.equipment.game_equipment_url.clone())
        })
        .ok_or(())?;
    let target = PublicEquipmentLinkTarget {
        lot_id,
        equipment_id: detail.equipment_id,
        world_id: detail.projection.world_id.clone(),
        projected_game_equipment_url,
    };
    target.validate().map_err(|_| ())?;
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
        (None, Some(live)) => {
            let mut mapped = map_live_lot(live.summary, Some(live.equipment_attributes));
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
    if raw.is_empty() || raw.len() > 128 {
        return Err(());
    }
    let decoded = URL_SAFE_NO_PAD.decode(raw).map_err(|_| ())?;
    if decoded.len() != 26
        || URL_SAFE_NO_PAD.encode(&decoded) != raw
        || decoded[0] != 1
        || decoded[1] != 3
    {
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
    if raw.is_empty() || raw.len() > 128 {
        return Err(());
    }
    let decoded = URL_SAFE_NO_PAD.decode(raw).map_err(|_| ())?;
    if decoded.len() != 54
        || URL_SAFE_NO_PAD.encode(&decoded) != raw
        || decoded[0] != 1
        || decoded[1] != 4
    {
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
    if raw.is_empty() || raw.len() > 128 {
        return Err(());
    }
    let decoded = URL_SAFE_NO_PAD.decode(raw).map_err(|_| ())?;
    if decoded.len() != 38
        || URL_SAFE_NO_PAD.encode(&decoded) != raw
        || decoded[0] != 1
        || decoded[1] != 5
    {
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
