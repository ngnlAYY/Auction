//! Admin 列表分页信封合并后，各列表原有的浏览器解码严格度必须保持不变。
//!
//! 九个分页名字共用两种信封：原本带 `deny_unknown_fields` 的五个仍拒绝未知字段，其余四个
//! 仍忽略未知字段；字段形状与序列化结果不变。

use auction_contracts::{
    AdminAuctionPaginationResponse, AdminCredentialPaginationResponse,
    AdminDeliveryPaginationResponse, AdminLotPaginationResponse, AdminMailPaginationResponse,
    AdminRealtimePaginationResponse, AdminRenamePaginationResponse,
    AdminSettlementCatalogPaginationResponse, AdminUserPaginationResponse, ApiResponse,
    HealthLiveData, HealthProbeResponse,
};
use serde::de::DeserializeOwned;

const PAGE: &str = r#"{"page":2,"page_size":20,"total":41}"#;
const PAGE_WITH_EXTRA: &str = r#"{"page":2,"page_size":20,"total":41,"cursor":"next"}"#;

fn accepts_extra<T: DeserializeOwned>() -> bool {
    serde_json::from_str::<T>(PAGE_WITH_EXTRA).is_ok()
}

#[test]
fn strict_lists_still_reject_unknown_pagination_fields() {
    assert!(!accepts_extra::<AdminAuctionPaginationResponse>());
    assert!(!accepts_extra::<AdminCredentialPaginationResponse>());
    assert!(!accepts_extra::<AdminDeliveryPaginationResponse>());
    assert!(!accepts_extra::<AdminLotPaginationResponse>());
    assert!(!accepts_extra::<AdminSettlementCatalogPaginationResponse>());
}

#[test]
fn lenient_lists_still_ignore_unknown_pagination_fields() {
    assert!(accepts_extra::<AdminMailPaginationResponse>());
    assert!(accepts_extra::<AdminRealtimePaginationResponse>());
    assert!(accepts_extra::<AdminRenamePaginationResponse>());
    assert!(accepts_extra::<AdminUserPaginationResponse>());
}

#[test]
fn pagination_wire_shape_is_unchanged() {
    let strict: AdminLotPaginationResponse =
        serde_json::from_str(PAGE).unwrap_or_else(|error| panic!("分页必须可解码: {error}"));
    let lenient: AdminUserPaginationResponse =
        serde_json::from_str(PAGE).unwrap_or_else(|error| panic!("分页必须可解码: {error}"));
    assert_eq!((strict.page, strict.page_size, strict.total), (2, 20, 41));
    assert_eq!(serde_json::to_string(&strict).ok().as_deref(), Some(PAGE));
    assert_eq!(serde_json::to_string(&lenient).ok().as_deref(), Some(PAGE));
}

#[test]
fn health_probe_envelope_keeps_the_success_shape() {
    let probe: HealthProbeResponse<HealthLiveData> =
        ApiResponse::ok("ok", HealthLiveData { live: true });
    assert_eq!(
        serde_json::to_string(&probe).ok().as_deref(),
        Some(r#"{"code":"ok","message":"ok","data":{"live":true}}"#)
    );
}
