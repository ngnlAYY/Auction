//! Admin 结算报表选择目录客户端。

use std::collections::HashSet;

use auction_contracts::{AdminSettlementCatalogResponse, ApiResponse};
use auction_time::Timestamp;

use super::{MAX_JSON_SAFE_INTEGER, invalid_response_error, response_error, send_get, valid_uuid};

// contracts DTO 拒绝未知字段；此处补充排序、去重与筛选回显等跨字段校验。

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
    page_size: i64,
) -> Result<AdminSettlementCatalogPage, String> {
    super::pagination::validate(page, page_size, 100, 100_000)?;
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.to_string()).append_pair("page_size", &page_size.to_string());
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
    decode_catalog(payload.data, &world_id, sequence, page, page_size)
        .map_err(invalid_response_error)
}

fn decode_catalog(
    data: AdminSettlementCatalogResponse,
    world_id: &str,
    sequence: Option<i64>,
    requested_page: i64,
    requested_size: i64,
) -> Result<AdminSettlementCatalogPage, &'static str> {
    let pagination = data.pagination;
    // 目录页大小上限 100 是本端点的特例，其余回显与条数约束与其他列表一致。
    if pagination.page_size > 100
        || !super::pagination::valid_page_echo(
            requested_page,
            requested_size,
            pagination.page,
            pagination.page_size,
            pagination.total,
            data.auctions.len(),
        )
    {
        return Err("settlement_catalog_pagination");
    }

    let mut auction_ids = HashSet::with_capacity(data.auctions.len());
    let mut periods = HashSet::with_capacity(data.auctions.len());
    let mut previous_key: Option<CatalogOrderKey<'_>> = None;
    // 校验阶段借用完整响应中的文本，成功后再转移字段，避免去重和排序键逐行复制字符串。
    for entry in &data.auctions {
        // 后端按微秒排序，只有时刻真正相等才使用后续排序键。
        let settled_at = Timestamp::parse_from_rfc3339(&entry.settled_at)
            .ok()
            .ok_or("settlement_catalog_time")?;
        if !valid_uuid(&entry.auction_id)
            || !matches!(entry.world_id.as_str(), "main" | "isekai")
            || entry.sequence <= 0
            || entry.sequence > MAX_JSON_SAFE_INTEGER
            || world_id != "all" && entry.world_id != world_id
            || sequence.is_some_and(|value| entry.sequence != value)
            || !auction_ids.insert(entry.auction_id.as_str())
            || !periods.insert((entry.world_id.as_str(), entry.sequence))
        {
            return Err("settlement_catalog_entry");
        }
        let key = CatalogOrderKey {
            settled_at,
            world_id: &entry.world_id,
            sequence: entry.sequence,
            auction_id: &entry.auction_id,
        };
        if previous_key
            .as_ref()
            .is_some_and(|previous| !catalog_order_valid(previous, &key, world_id != "all"))
        {
            return Err("settlement_catalog_order");
        }
        previous_key = Some(key);
    }
    let auctions = data
        .auctions
        .into_iter()
        .map(|entry| AdminSettlementCatalogEntry {
            auction_id: entry.auction_id,
            world_id: entry.world_id,
            sequence: entry.sequence,
            settled_at: entry.settled_at,
        })
        .collect();

    Ok(AdminSettlementCatalogPage {
        auctions,
        page: pagination.page,
        page_size: pagination.page_size,
        total: pagination.total,
    })
}

struct CatalogOrderKey<'a> {
    settled_at: Timestamp,
    world_id: &'a str,
    sequence: i64,
    auction_id: &'a str,
}

fn catalog_order_valid(
    previous: &CatalogOrderKey<'_>,
    current: &CatalogOrderKey<'_>,
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AdminSettlementCatalogResponse, decode_catalog};

    fn catalog_within_same_millisecond() -> AdminSettlementCatalogResponse {
        serde_json::from_value(json!({
            "auctions": [
                {
                    "auction_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
                    "world_id": "main",
                    "sequence": 9,
                    "settled_at": "2026-11-06T10:00:00.123999Z"
                },
                {
                    "auction_id": "2c9e63a1-7f4b-4a0e-b3d5-9f8c1e2d4b60",
                    "world_id": "isekai",
                    "sequence": 9,
                    "settled_at": "2026-11-06T10:00:00.123001Z"
                }
            ],
            "pagination": {"page": 1, "page_size": 10, "total": 2}
        }))
        .unwrap()
    }

    #[test]
    fn cross_world_catalog_keeps_submillisecond_order() {
        let mut catalog = catalog_within_same_millisecond();
        assert!(decode_catalog(catalog.clone(), "all", None, 1, 10).is_ok());
        catalog.auctions.swap(0, 1);
        assert_eq!(decode_catalog(catalog, "all", None, 1, 10), Err("settlement_catalog_order"));
    }

    #[test]
    fn cross_world_catalog_breaks_exact_time_ties_by_world() {
        let mut catalog = catalog_within_same_millisecond();
        catalog.auctions[1].settled_at = catalog.auctions[0].settled_at.clone();
        assert_eq!(
            decode_catalog(catalog.clone(), "all", None, 1, 10),
            Err("settlement_catalog_order")
        );
        catalog.auctions.swap(0, 1);
        assert!(decode_catalog(catalog, "all", None, 1, 10).is_ok());
    }

    #[test]
    fn catalog_decodes_backend_contract_fixtures_at_microsecond_precision() {
        use auction_contracts::ApiResponse;

        let all: ApiResponse<AdminSettlementCatalogResponse> = serde_json::from_str(include_str!(
            "../../../auction-contracts/tests/fixtures/admin/settlement-catalog.json"
        ))
        .expect("跨世界目录样本");
        let mut reversed = all.data.clone();
        reversed.auctions.reverse();
        let catalog = decode_catalog(all.data, "all", None, 1, 50).expect("跨世界结算目录");
        assert_eq!(catalog.auctions[0].world_id, "main");
        assert!(decode_catalog(reversed, "all", None, 1, 50).is_err());

        let one: ApiResponse<AdminSettlementCatalogResponse> = serde_json::from_str(include_str!(
            "../../../auction-contracts/tests/fixtures/admin/settlement-catalog-world.json"
        ))
        .expect("单世界目录样本");
        let catalog = decode_catalog(one.data, "isekai", None, 1, 50).expect("单世界目录");
        assert_eq!(catalog.auctions[0].world_id, "isekai");
    }
}
