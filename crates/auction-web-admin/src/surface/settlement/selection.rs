//! 结算目录的场次解析与详情报告身份核验。

use crate::api::{AdminSettlementCatalogEntry, AdminSettlementCatalogPage, AdminSettlementReport};

/// 目录对当前选择的解析结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogSelection<'a> {
    Resolving,
    Matched(&'a AdminSettlementCatalogEntry),
    Contradictory,
}

/// 根据当前目录页与期号补查结果解析场次。
pub(super) fn resolve_catalog_selection<'a>(
    page: Option<&'a AdminSettlementCatalogPage>,
    lookup: Option<&'a AdminSettlementCatalogPage>,
    auction_id: &str,
    sequence: Option<i64>,
) -> CatalogSelection<'a> {
    let Some(page) = page else {
        return CatalogSelection::Resolving;
    };
    let Some(sequence) = sequence else {
        return if auction_id.is_empty() {
            page.auctions.first().map_or(CatalogSelection::Resolving, CatalogSelection::Matched)
        } else {
            CatalogSelection::Resolving
        };
    };

    // 有 UUID 时只能解析到该记录。不同世界可以复用同一个期号，不能取查询的第一项代替。
    if let Some(entry) = page.auctions.iter().find(|entry| {
        if auction_id.is_empty() {
            entry.sequence == sequence
        } else {
            entry.auction_id == auction_id
        }
    }) {
        return if entry.sequence == sequence {
            CatalogSelection::Matched(entry)
        } else {
            CatalogSelection::Contradictory
        };
    }
    lookup.map_or(CatalogSelection::Resolving, |lookup| {
        lookup
            .auctions
            .iter()
            .find(|entry| {
                entry.sequence == sequence
                    && (auction_id.is_empty() || entry.auction_id == auction_id)
            })
            .map_or(CatalogSelection::Contradictory, CatalogSelection::Matched)
    })
}

/// 只展示与目录命中同一已结算场次的报告。
pub(super) fn report_for_selection<'a>(
    selection: CatalogSelection<'_>,
    report: Option<&'a AdminSettlementReport>,
) -> Result<Option<&'a AdminSettlementReport>, &'static str> {
    let (CatalogSelection::Matched(entry), Some(report)) = (selection, report) else {
        return Ok(None);
    };
    if report.auction.id != entry.auction_id
        || report.auction.world_id != entry.world_id
        || report.auction.sequence != entry.sequence
        || report.auction.status != "settled"
    {
        return Err("invalid_response");
    }
    Ok(Some(report))
}
