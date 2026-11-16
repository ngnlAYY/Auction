//! Public 只读目录的 DTO 映射：拍卖列表与当前场次、lot 列表与详情、竞价日志与价格历史。
//! 投影由应用层校验；这里只做 wire 字段映射和装备链接所有权转移。

use auction_application::auction::{
    CurrentAuctionSnapshot, PublicAuctionPage, PublicAuctionProjection, PublicAuctionSummary,
    PublicBidLogEntry, PublicBidLogPage, PublicEquipment, PublicFinalRestriction,
    PublicLiveLotSummary, PublicLotDetail, PublicLotPage, PublicPreviewLotDetail,
    PublicPreviewLotSummary, PublicPriceHistoryEntry, PublicPriceHistoryPage,
    PublicPriceHistoryStats,
};
use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::public::PublicEquipmentLinkTarget;
use auction_contracts::{
    AuctionListResponse, AuctionSummaryResponse, AuctionSummaryV2Response,
    CurrentAuctionItemResponse, PublicBidLogEntryResponse, PublicBidLogListResponse,
    PublicDefeatedProxyResponse, PublicEquipmentResponse, PublicEquipmentStatResponse,
    PublicFinalRestrictionResponse, PublicLiveLotResponse, PublicLotDetailItemResponse,
    PublicLotEnvelopeResponse, PublicLotListItemsResponse, PublicPreviewEquipmentResponse,
    PublicPreviewLotDetailResponse, PublicPreviewLotResponse, PublicPriceHistoryEntryResponse,
    PublicPriceHistoryListResponse, PublicPriceHistoryStatsResponse,
};
use uuid::Uuid;

use crate::diagnostics;
use crate::mapping::{
    encode_public_bid_log_cursor, encode_public_cursor, encode_public_price_history_cursor,
    public_timestamp,
};

pub(super) fn map_auction_summary(summary: PublicAuctionSummary) -> AuctionSummaryResponse {
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
pub(super) fn map_auction_summary_v2(
    mut summary: PublicAuctionSummary,
) -> AuctionSummaryV2Response {
    let forum_thread_url =
        summary.forum_link.take().map(auction_application::forum::PublicForumLink::into_url);
    AuctionSummaryV2Response { auction: map_auction_summary(summary), forum_thread_url }
}

/// `include_proxy_default_mode` 为假时 DTO 省略该键，条目保持 v1 原字段集合。
pub(super) fn map_current_auction(
    auction: CurrentAuctionSnapshot,
    include_proxy_default_mode: bool,
) -> CurrentAuctionItemResponse {
    let proxy_default_mode =
        include_proxy_default_mode.then(|| auction.proxy_default_mode.as_str().to_owned());
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
        proxy_default_mode,
    }
}

/// lot 列表的字段投影，由查询参数 `fields` 选择。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LotListFields {
    /// 缺省：v1 原字段集合，已发布调用方看到的正文逐字节不变。
    Full,
    /// `fields=summary`：live 行省略装备 `stats` 与 `info_summary`，属性明细由 lot 详情提供。
    Summary,
}

pub(super) fn map_lot_page(
    page: PublicLotPage,
    page_number: i64,
    game_equipment_urls: Vec<String>,
    fields: LotListFields,
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
                    if fields == LotListFields::Summary {
                        // 首字母在 map_equipment 内由 stats 键推导，只能在映射之后清空；avg_roll
                        // 来自存储投影。空 map 与空串由 DTO 省略，其余字段与完整投影相同。
                        mapped.equipment.stats.clear();
                        mapped.equipment.info_summary.clear();
                    }
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
pub(super) fn lot_page_equipment_targets(
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
pub(super) fn lot_detail_equipment_target(
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

pub(super) fn map_lot_detail(
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

fn map_lot_envelope(projection: PublicAuctionProjection) -> PublicLotEnvelopeResponse {
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

fn map_final_restriction(restriction: PublicFinalRestriction) -> PublicFinalRestrictionResponse {
    PublicFinalRestrictionResponse {
        new_slow_proxy_allowed: restriction.new_slow_proxy_allowed,
        existing_slow_proxy_converted: restriction.existing_slow_proxy_converted,
    }
}

fn map_live_lot(
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

fn map_preview_lot(lot: PublicPreviewLotSummary) -> PublicPreviewLotResponse {
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

fn map_preview_detail(detail: PublicPreviewLotDetail) -> PublicPreviewLotDetailResponse {
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

fn map_equipment(
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

pub(super) fn valid_lot_detail(detail: &PublicLotDetail, lot_id: Uuid) -> bool {
    detail.validate(lot_id).is_ok()
}

pub(super) fn map_auction_page<T>(
    page: PublicAuctionPage,
    page_number: i64,
    cursor: Option<String>,
    map_summary: impl FnMut(PublicAuctionSummary) -> T,
) -> AuctionListResponse<T> {
    let auctions = page.entries.into_iter().map(map_summary).collect();
    let next_cursor = page.next_cursor.as_ref().map(encode_public_cursor);
    let has_more = cursor.as_ref().map(|_| page.has_more);
    AuctionListResponse {
        auctions,
        page: cursor.is_none().then_some(page_number),
        page_size: page.limit,
        total: page.total,
        cursor,
        next_cursor,
        has_more,
    }
}

/// `include_defeated_proxy` 为假时条目保持原字段闭集，落败授权一律不输出。
pub(super) fn map_bid_log_page(
    page: PublicBidLogPage,
    page_number: i64,
    cursor: Option<String>,
    include_defeated_proxy: bool,
) -> PublicBidLogListResponse {
    let cursor_requested = cursor.is_some();
    let next_cursor = page.next_cursor.as_ref().map(encode_public_bid_log_cursor);
    PublicBidLogListResponse {
        bids: page
            .entries
            .into_iter()
            .map(|entry| map_bid_log_entry(entry, include_defeated_proxy))
            .collect(),
        page: (!cursor_requested).then_some(page_number),
        page_size: page.limit,
        total: page.total,
        cursor,
        next_cursor: cursor_requested.then_some(next_cursor).flatten(),
        has_more: cursor_requested.then_some(page.has_more),
    }
}

fn map_bid_log_entry(
    entry: PublicBidLogEntry,
    include_defeated_proxy: bool,
) -> PublicBidLogEntryResponse {
    let source_post_url = forum_post_url(&entry);
    PublicBidLogEntryResponse {
        id: entry.id.to_string(),
        auction_id: entry.auction_id.to_string(),
        lot_id: entry.lot_id.to_string(),
        lot_number: entry.lot_number,
        amount: entry.amount,
        equipment_name: entry.equipment_name,
        equipment_category: entry.equipment_category,
        bidder_name: entry.bidder_name,
        source: entry.source,
        source_post_number: entry.source_post_number.map(i64::from),
        source_post_url,
        valid: entry.valid,
        accepted_state_version: entry.accepted_state_version,
        fact_index: i64::from(entry.fact_index),
        mechanism: entry.mechanism,
        created_at: public_timestamp(entry.created_at),
        defeated_proxy: entry.defeated_proxy.filter(|_| include_defeated_proxy).map(|defeated| {
            PublicDefeatedProxyResponse {
                bidder_name: defeated.bidder_name,
                maximum_amount: defeated.maximum_amount,
            }
        }),
    }
}

fn forum_post_url(entry: &PublicBidLogEntry) -> Option<String> {
    if entry.mechanism != "ordinary"
        || entry.source.as_deref() != Some("forum")
        || entry.source_post_number.is_none()
    {
        return None;
    }
    let thread_id = entry.source_thread_id.as_deref().filter(|value| canonical_forum_id(value))?;
    let post_id = entry.source_post_id.as_deref().filter(|value| canonical_forum_id(value))?;
    Some(format!(
        "https://forums.e-hentai.org/index.php?showtopic={thread_id}&p={post_id}#entry{post_id}"
    ))
}

fn canonical_forum_id(value: &str) -> bool {
    auction_domain::valid_positive_decimal(value, 32)
}

pub(super) fn map_price_history_page(
    page: PublicPriceHistoryPage,
    page_number: i64,
    cursor: Option<String>,
) -> PublicPriceHistoryListResponse {
    let cursor_requested = cursor.is_some();
    let next_cursor = page.next_cursor.as_ref().map(encode_public_price_history_cursor);
    PublicPriceHistoryListResponse {
        rows: page.entries.into_iter().map(map_price_history_entry).collect(),
        stats: page.stats.map(map_price_history_stats),
        page: (!cursor_requested).then_some(page_number),
        page_size: page.limit,
        total: page.total,
        cursor,
        next_cursor: cursor_requested.then_some(next_cursor).flatten(),
        has_more: cursor_requested.then_some(page.has_more),
    }
}

fn map_price_history_entry(entry: PublicPriceHistoryEntry) -> PublicPriceHistoryEntryResponse {
    PublicPriceHistoryEntryResponse {
        lot_id: entry.lot_id.to_string(),
        auction_id: entry.auction_id.to_string(),
        world_id: entry.world_id,
        auction_sequence: entry.auction_sequence,
        lot_number: entry.lot_number,
        equipment_name: entry.equipment_name,
        equipment_category: entry.equipment_category,
        equipment_level: entry.equipment_level,
        buyer_name: entry.buyer_name,
        seller_name: entry.seller_name,
        price: entry.price,
        closed_at: public_timestamp(entry.closed_at),
    }
}

fn map_price_history_stats(stats: PublicPriceHistoryStats) -> PublicPriceHistoryStatsResponse {
    PublicPriceHistoryStatsResponse {
        count: stats.count,
        min: stats.minimum,
        max: stats.maximum,
        median: stats.median,
        median_exact: stats.median_exact,
    }
}
