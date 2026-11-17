//! 结算退件列表的查询参数解析：分页有界、筛选全部精确且属于闭集，任一非法值整页失败关闭，
//! 不做部分忽略。

use std::collections::BTreeMap;

use auction_application::settlement::{
    MAX_SETTLEMENT_RETURN_OFFSET, MAX_SETTLEMENT_RETURN_PAGE_SIZE, SettlementReturnListRequest,
    SettlementReturnMatch,
};
use auction_domain::{SettlementObligationKind, SettlementReturnCaseStatus};
use axum::http::Uri;

use crate::admin::delivery::{parse_optional_reference, parse_optional_timestamp};
use crate::query::{page_offset, parse_positive_i64, parse_query};

/// 列表缺省页长。
const DEFAULT_PAGE_SIZE: i64 = 50;

/// 列表查询键闭集。
const LIST_KEYS: [&str; 10] = [
    "page",
    "page_size",
    "status",
    "match_state",
    "obligation_kind",
    "world",
    "open_only",
    "received_from",
    "received_to",
    "reference",
];

/// 已校验的列表请求与需要回显的页码。
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ListQuery {
    pub(super) page: i64,
    pub(super) request: SettlementReturnListRequest,
}

/// 解析列表查询；空值、未登记键、重复键、闭集外取值、越界页长或偏移都返回 `Err`。
pub(super) fn list_query(uri: &Uri) -> Result<ListQuery, ()> {
    let query = parse_query(uri, &LIST_KEYS)?;
    // 浏览器省略未设置的筛选；空串不当作“未设置”，避免与精确筛选混淆。
    if query.values().any(String::is_empty) {
        return Err(());
    }
    let page = parse_positive_i64(query.get("page"), 1, None)?;
    let page_size = parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_SETTLEMENT_RETURN_PAGE_SIZE),
    )?;
    let offset = page_offset(page, page_size, Some(MAX_SETTLEMENT_RETURN_OFFSET)).ok_or(())?;
    let open_only = match query.get("open_only").map(String::as_str) {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err(()),
    };
    let request = SettlementReturnListRequest {
        world: closed(&query, "world", |value| {
            matches!(value, "main" | "isekai").then(|| value.to_owned())
        })?,
        match_state: closed(&query, "match_state", SettlementReturnMatch::from_name)?,
        status: closed(&query, "status", SettlementReturnCaseStatus::from_name)?,
        obligation_kind: closed(&query, "obligation_kind", SettlementObligationKind::from_name)?,
        open_only,
        received_from: parse_optional_timestamp(&query, "received_from")?,
        received_to: parse_optional_timestamp(&query, "received_to")?,
        reference: parse_optional_reference(&query, "reference")?,
        limit: page_size,
        offset,
    };
    if !request.validate() {
        return Err(());
    }
    Ok(ListQuery { page, request })
}

/// 可选闭集筛选：缺省为 `None`，出现时必须能解析为闭集取值。
fn closed<T>(
    query: &BTreeMap<&str, String>,
    key: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Option<T>, ()> {
    query.get(key).map(|value| parse(value).ok_or(())).transpose()
}

#[cfg(test)]
mod tests {
    use auction_application::settlement::SettlementReturnMatch;
    use auction_domain::{SettlementObligationKind, SettlementReturnCaseStatus};
    use axum::http::Uri;

    use super::{ListQuery, list_query};

    fn parse(query: &str) -> Result<ListQuery, ()> {
        let uri: Uri =
            format!("/api/v1/admin/settlement-returns{query}").parse().expect("测试 URI");
        list_query(&uri)
    }

    /// 缺省第一页 50 条、不限状态；全部筛选按闭集解析为精确值。
    #[test]
    fn defaults_and_closed_filters_parse() {
        let defaults = parse("").expect("缺省查询");
        assert_eq!((defaults.page, defaults.request.limit, defaults.request.offset), (1, 50, 0));
        assert!(!defaults.request.open_only);

        let parsed = parse(
            "?page=3&page_size=20&status=take_unknown&match_state=manual_reconcile\
             &obligation_kind=seller_payout&world=isekai&open_only=true\
             &received_from=2026-11-01T00:00:00Z&received_to=2026-11-02T00:00:00%2B08:00\
             &reference=0f8fad5b-d9cb-469f-a165-70867728950e",
        )
        .expect("完整筛选");
        assert_eq!((parsed.page, parsed.request.limit, parsed.request.offset), (3, 20, 40));
        assert_eq!(parsed.request.status, Some(SettlementReturnCaseStatus::TakeUnknown));
        assert_eq!(parsed.request.match_state, Some(SettlementReturnMatch::ManualReconcile));
        assert_eq!(parsed.request.obligation_kind, Some(SettlementObligationKind::SellerPayout));
        assert_eq!(parsed.request.world.as_deref(), Some("isekai"));
        assert!(parsed.request.open_only);
        assert!(parsed.request.reference.is_some());
        assert!(parsed.request.received_from.is_some() && parsed.request.received_to.is_some());
    }

    /// 页长上限 200、偏移上限 10000；越界、空值、闭集外取值、重复或未登记的键都整页拒绝。
    #[test]
    fn invalid_or_unbounded_queries_fail_closed() {
        assert!(parse("?page_size=200").is_ok());
        assert!(parse("?page=51&page_size=200").is_ok(), "偏移恰为 10000");
        for invalid in [
            "?page_size=201",
            "?page_size=0",
            "?page=52&page_size=200",
            "?page=0",
            "?status=",
            "?status=open",
            "?status=TAKE_UNKNOWN",
            "?match_state=bound",
            "?obligation_kind=payout",
            "?world=other",
            "?open_only=1",
            "?reference=0F8FAD5B-D9CB-469F-A165-70867728950E",
            "?reference=00000000-0000-0000-0000-000000000000",
            "?received_from=2026-11-02",
            "?received_from=2026-11-03T00:00:00Z&received_to=2026-11-02T00:00:00Z",
            "?status=held&status=held",
            "?recipient=Someone",
        ] {
            assert!(parse(invalid).is_err(), "应拒绝 {invalid}");
        }
    }
}
