//! 组合已有只读 API，完整求出一个世界当前公开目录与账号标记的交集。

use std::collections::{BTreeMap, BTreeSet};

use crate::api::{
    CatalogRow, CurrentAuction, LotEnvelopeRows, fetch_current_auction, fetch_current_lots,
    fetch_lot_watches,
};

/// 当前场次及其完整公开标记结果；总数由真实匹配行数产生。
#[derive(Clone, PartialEq)]
pub(super) struct WatchedAuction {
    pub(super) auction: CurrentAuction,
    pub(super) rows: Vec<CatalogRow>,
}

const PAGE_SIZE: i64 = 200;
// 既有目录 API 允许 offset <= 10000；末页从 10000 起，仍以 200 条为界。
const MAX_CATALOG_ROWS: i64 = 10_200;

/// 读取一个固定账号与世界的完整公开标记，返回前复核当前场次和集合版本。
pub(super) async fn load(user_id: &str, world: &str) -> Result<Option<WatchedAuction>, ()> {
    let Some(auction) = fetch_current_auction(world.to_owned()).await.map_err(|_| ())? else {
        return Ok(None);
    };
    let watches = fetch_lot_watches(user_id, &auction.id).await.map_err(|_| ())?;
    let expected = watch_versions(&watches);
    let watched = watches
        .watches
        .iter()
        .filter(|watch| watch.watched)
        .map(|watch| watch.lot_id.clone())
        .collect::<BTreeSet<_>>();
    // 未标记时不读取任何装备目录，场次和账号集合仍在返回前重新核对。
    let rows =
        if watched.is_empty() { Vec::new() } else { load_catalog(&auction, &watched).await? };
    let current = fetch_current_auction(world.to_owned()).await.map_err(|_| ())?;
    let current_watches = fetch_lot_watches(user_id, &auction.id).await.map_err(|_| ())?;
    if current.as_ref() != Some(&auction) || watch_versions(&current_watches) != expected {
        return Err(());
    }
    Ok(Some(WatchedAuction { auction, rows }))
}

fn watch_versions(
    watches: &auction_contracts::PublicLotWatchesResponse,
) -> BTreeMap<String, (bool, i64)> {
    watches
        .watches
        .iter()
        .map(|watch| (watch.lot_id.clone(), (watch.watched, watch.version)))
        .collect()
}

async fn load_catalog(
    auction: &CurrentAuction,
    watched: &BTreeSet<String>,
) -> Result<Vec<CatalogRow>, ()> {
    let mut page_number = 1_i64;
    let mut expected_total = None;
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    loop {
        let envelope = fetch_current_lots(auction.id.clone(), None, None, page_number, PAGE_SIZE)
            .await
            .map_err(|_| ())?;
        let total = envelope.total.ok_or(())?;
        if envelope.auction_id != auction.id
            || envelope.auction_status != auction.status
            || envelope.preview_revision != auction.preview_revision
            || envelope.presentation_mode != auction.presentation_mode
            || envelope.bidding_open != auction.bidding_open
            || envelope.opens_at != auction.opens_at
            || envelope.page != Some(page_number)
            || envelope.page_size != Some(PAGE_SIZE)
            || !(0..=MAX_CATALOG_ROWS).contains(&total)
            || expected_total.is_some_and(|expected| expected != total)
        {
            return Err(());
        }
        expected_total = Some(total);
        let offset = (page_number - 1).checked_mul(PAGE_SIZE).ok_or(())?;
        let rows = envelope.rows();
        let expected_rows = (total - offset).clamp(0, PAGE_SIZE);
        if i64::try_from(rows.len()).map_err(|_| ())? != expected_rows {
            return Err(());
        }
        for row in rows {
            if !seen.insert(row.lot_id.clone()) {
                return Err(());
            }
            if watched.contains(&row.lot_id) {
                result.push(row);
            }
        }
        if offset + PAGE_SIZE >= total {
            // 撤回等非公开标的沿用目录可见性，不从旧缓存补名称或伪造详情。
            return Ok(result);
        }
        page_number = page_number.checked_add(1).ok_or(())?;
    }
}
