//! Admin 结算报表选择目录客户端。

use std::collections::HashSet;

use auction_contracts::AdminSettlementCatalogResponse;

use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, TABLE_PAGE_SIZE, invalid_response_error, parse_admin_time,
    response_error, send_get, valid_uuid,
};

// 本地 Strict 镜像已删除：contracts 的 `AdminSettlementCatalogResponse`（含 entry 与
// pagination）已带 `deny_unknown_fields`，字段集与旧闭集 decoder 一致；本文件只保留排序、
// 去重与筛选回显等跨字段谓词。

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminSettlementCatalogEntry {
    pub auction_id: String,
    pub world_id: String,
    pub sequence: i64,
    pub settled_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminSettlementCatalogPage {
    pub auctions: Vec<AdminSettlementCatalogEntry>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// 读取已结算拍卖目录；`world_id = all` 只存在于页面状态，不进入服务端查询。
pub(crate) async fn fetch_admin_settlement_catalog(
    world_id: String,
    sequence: Option<i64>,
    page: i64,
) -> Result<AdminSettlementCatalogPage, String> {
    let page = page.max(1);
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.to_string())
        .append_pair("page_size", &TABLE_PAGE_SIZE.to_string());
    if world_id != "all" {
        query.append_pair("world_id", &world_id);
    }
    if let Some(sequence) = sequence {
        query.append_pair("sequence", &sequence.to_string());
    }
    let url = format!("/api/v1/admin/settlement-catalog?{}", query.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminSettlementCatalogResponse> =
        response.json().await.map_err(invalid_response_error)?;
    if payload.code != "ok" || payload.message.trim().is_empty() {
        return Err(invalid_response_error("settlement_catalog_envelope"));
    }
    decode_catalog(payload.data, &world_id, sequence, page).map_err(invalid_response_error)
}

fn decode_catalog(
    data: AdminSettlementCatalogResponse,
    world_id: &str,
    sequence: Option<i64>,
    requested_page: i64,
) -> Result<AdminSettlementCatalogPage, &'static str> {
    let pagination = data.pagination;
    if pagination.page != requested_page
        || pagination.page_size != TABLE_PAGE_SIZE
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&pagination.page)
        || !(1..=100).contains(&pagination.page_size)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&pagination.total)
        || i64::try_from(data.auctions.len()).is_err()
        || data.auctions.len() as i64 > pagination.page_size
    {
        return Err("settlement_catalog_pagination");
    }

    let mut auction_ids = HashSet::with_capacity(data.auctions.len());
    let mut periods = HashSet::with_capacity(data.auctions.len());
    let mut previous_key: Option<CatalogOrderKey> = None;
    let auctions = data
        .auctions
        .into_iter()
        .map(|entry| {
            let settled_at = parse_admin_time(&entry.settled_at)
                .filter(|value| value.is_finite())
                .ok_or("settlement_catalog_time")?;
            if !valid_uuid(&entry.auction_id)
                || !matches!(entry.world_id.as_str(), "main" | "isekai")
                || entry.sequence <= 0
                || entry.sequence > MAX_JSON_SAFE_INTEGER
                || world_id != "all" && entry.world_id != world_id
                || sequence.is_some_and(|value| entry.sequence != value)
                || !auction_ids.insert(entry.auction_id.clone())
                || !periods.insert((entry.world_id.clone(), entry.sequence))
            {
                return Err("settlement_catalog_entry");
            }
            let key = CatalogOrderKey {
                settled_at,
                world_id: entry.world_id.clone(),
                sequence: entry.sequence,
                auction_id: entry.auction_id.clone(),
            };
            if previous_key
                .as_ref()
                .is_some_and(|previous| !catalog_order_valid(previous, &key, world_id != "all"))
            {
                return Err("settlement_catalog_order");
            }
            previous_key = Some(key);
            Ok(AdminSettlementCatalogEntry {
                auction_id: entry.auction_id,
                world_id: entry.world_id,
                sequence: entry.sequence,
                settled_at: entry.settled_at,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(AdminSettlementCatalogPage {
        auctions,
        page: pagination.page,
        page_size: pagination.page_size,
        total: pagination.total,
    })
}

struct CatalogOrderKey {
    settled_at: f64,
    world_id: String,
    sequence: i64,
    auction_id: String,
}

fn catalog_order_valid(
    previous: &CatalogOrderKey,
    current: &CatalogOrderKey,
    single_world: bool,
) -> bool {
    if single_world && previous.sequence != current.sequence {
        return previous.sequence > current.sequence;
    }
    if previous.settled_at != current.settled_at {
        return previous.settled_at > current.settled_at;
    }
    if !single_world && previous.world_id != current.world_id {
        return previous.world_id < current.world_id;
    }
    if previous.sequence != current.sequence {
        return previous.sequence > current.sequence;
    }
    previous.auction_id >= current.auction_id
}
