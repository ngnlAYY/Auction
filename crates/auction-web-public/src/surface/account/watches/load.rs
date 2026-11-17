//! 组合已有只读 API，完整求出一个世界当前公开目录与账号标记的交集。

use std::collections::{BTreeMap, BTreeSet};

use auction_contracts::MAX_PUBLIC_PAGE_SIZE;

use crate::api::{
    CatalogLevelFilter, CatalogRow, CurrentAuction, LotEnvelopeRows, fetch_current_auction,
    fetch_current_lots, fetch_lot_watches,
};

/// 当前场次及其完整公开标记结果；总数由真实匹配行数产生。
#[derive(PartialEq)]
pub(super) struct WatchedAuction {
    pub(super) auction: CurrentAuction,
    pub(super) rows: Vec<CatalogRow>,
}

/// 按 Public 页长上限逐页读取整份目录。
const PAGE_SIZE: i64 = MAX_PUBLIC_PAGE_SIZE;
/// 既有目录 API 允许 offset <= 10000；末页从 10000 起，仍以一页为界。
const MAX_CATALOG_ROWS: i64 = 10_000 + PAGE_SIZE;

/// 按 [`PAGE_SIZE`] 逐页读取时，`total` 行目录第 `page_number` 页应有的行数，以及读完本页后是否
/// 已读完整份目录；页码非正、总数为负或超出 offset 上限可达的行数时为 `None`。
fn catalog_page_shape(page_number: i64, total: i64) -> Option<(i64, bool)> {
    if page_number < 1 || !(0..=MAX_CATALOG_ROWS).contains(&total) {
        return None;
    }
    let offset = (page_number - 1).checked_mul(PAGE_SIZE)?;
    Some(((total - offset).clamp(0, PAGE_SIZE), offset.saturating_add(PAGE_SIZE) >= total))
}

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
        .map(|watch| watch.lot_id.as_str())
        .collect::<BTreeSet<_>>();
    // 未标记时不读取任何装备目录，场次和账号集合仍在返回前重新核对。
    let rows =
        if watched.is_empty() { Vec::new() } else { load_catalog(&auction, &watched).await? };
    let (current, current_watches) = futures_util::join!(
        fetch_current_auction(world.to_owned()),
        fetch_lot_watches(user_id, &auction.id),
    );
    let current = current.map_err(|_| ())?;
    let current_watches = current_watches.map_err(|_| ())?;
    if current.as_ref() != Some(&auction) || watch_versions(&current_watches) != expected {
        return Err(());
    }
    Ok(Some(WatchedAuction { auction, rows }))
}

fn watch_versions(
    watches: &auction_contracts::PublicLotWatchesResponse,
) -> BTreeMap<&str, (bool, i64)> {
    watches
        .watches
        .iter()
        .map(|watch| (watch.lot_id.as_str(), (watch.watched, watch.version)))
        .collect()
}

async fn load_catalog(
    auction: &CurrentAuction,
    watched: &BTreeSet<&str>,
) -> Result<Vec<CatalogRow>, ()> {
    let mut page_number = 1_i64;
    let mut expected_total = None;
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    loop {
        let mut envelope = fetch_current_lots(
            auction.id.clone(),
            None,
            None,
            CatalogLevelFilter::default(),
            page_number,
            PAGE_SIZE,
        )
        .await
        .map_err(|_| ())?;
        let total = envelope.total.ok_or(())?;
        let (expected_rows, last_page) = catalog_page_shape(page_number, total).ok_or(())?;
        if envelope.auction_id != auction.id
            || envelope.auction_status != auction.status
            || envelope.preview_revision != auction.preview_revision
            || envelope.presentation_mode != auction.presentation_mode
            || envelope.bidding_open != auction.bidding_open
            || envelope.opens_at != auction.opens_at
            || envelope.page != Some(page_number)
            || envelope.page_size != Some(PAGE_SIZE)
            || expected_total.is_some_and(|expected| expected != total)
        {
            return Err(());
        }
        expected_total = Some(total);
        let rows = envelope.take_rows();
        if i64::try_from(rows.len()).map_err(|_| ())? != expected_rows {
            return Err(());
        }
        for row in rows {
            if !seen.insert(row.lot_id.clone()) {
                return Err(());
            }
            if watched.contains(row.lot_id.as_str()) {
                result.push(row);
            }
        }
        if last_page {
            // 撤回等非公开标的沿用目录可见性，不从旧缓存补名称或伪造详情。
            return Ok(result);
        }
        page_number = page_number.checked_add(1).ok_or(())?;
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_CATALOG_ROWS, PAGE_SIZE, catalog_page_shape};

    /// 按标记页的规则逐页读到最后一页，返回读取的页数与行数；某页形状非法时为 `None`。
    fn read_whole_catalog(total: i64) -> Option<(i64, i64)> {
        let mut rows = 0;
        for page_number in 1..=MAX_CATALOG_ROWS / PAGE_SIZE + 1 {
            let (page_rows, last_page) = catalog_page_shape(page_number, total)?;
            rows += page_rows;
            if last_page {
                return Some((page_number, rows));
            }
        }
        None
    }

    #[test]
    fn whole_catalog_is_read_page_by_page_at_the_public_maximum() {
        assert_eq!((PAGE_SIZE, MAX_CATALOG_ROWS), (200, 10_200));
        // 空目录、不满一页、恰好整页、跨页与 offset 上限上的末页：读到全部行，且不多读一页。
        for (total, pages) in
            [(0, 1), (1, 1), (200, 1), (201, 2), (401, 3), (10_000, 50), (10_200, 51)]
        {
            assert_eq!(read_whole_catalog(total), Some((pages, total)), "total={total}");
        }
        // 每页按上限读满，只有末页按剩余行数。
        assert_eq!(catalog_page_shape(2, 401), Some((200, false)));
        assert_eq!(catalog_page_shape(3, 401), Some((1, true)));
        // offset 上限之外的行读不到：总数超出可读范围、负数或页码非正都失败关闭。
        for (page_number, total) in [(1, 10_201), (1, -1), (0, 10), (-3, 10)] {
            assert_eq!(catalog_page_shape(page_number, total), None, "{page_number}/{total}");
        }
    }
}
