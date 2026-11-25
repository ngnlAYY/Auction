//! Public 实时查询目标和旧版十进制参数解析。

use std::collections::BTreeMap;

use auction_application::realtime::MAX_PUBLIC_EVENT_PAGE_SIZE;
use uuid::Uuid;

use crate::query::parse_public_uuid;

pub(super) const DEFAULT_PUBLIC_EVENT_PAGE_SIZE: i64 = 100;

// 事件回放沿用旧版规范十进制解析与应用端口的上限；该上限必须等于 Public 列表统一上限。
const _: () = assert!(MAX_PUBLIC_EVENT_PAGE_SIZE == auction_contracts::MAX_PUBLIC_PAGE_SIZE);

/// `GET /api/v1/events` 的查询参数白名单；其余键（含 `scope`）按未知参数拒绝。
pub(super) const LIST_QUERY_KEYS: &[&str] = &["auction_id", "after", "page_size"];

/// `/api/v1/ws` 的查询参数白名单：连接只能指向单一拍卖流。
pub(super) const WEBSOCKET_QUERY_KEYS: &[&str] = &["auction_id", "independent_build"];

/// 客户端能力声明：值只能是 `1`；声明后 hello 才可能带 `independent_build`。
const INDEPENDENT_BUILD_PARAM: &str = "independent_build";

/// 单拍卖流的 stream 名；回放页校验与 Hub 订阅共用同一格式。
pub(super) fn auction_stream_key(auction_id: Uuid) -> String {
    format!("auction:{auction_id}")
}

/// v1 实时边界只有单拍卖目标，必须携带规范 `auction_id`。
pub(super) fn parse_http_target(query: &BTreeMap<&str, String>) -> Result<Uuid, ()> {
    let Some(auction_id) = query.get("auction_id") else {
        return Err(());
    };
    parse_auction_id(auction_id)
}

pub(super) fn parse_websocket_target(query: &BTreeMap<&str, String>) -> Result<Uuid, ()> {
    // 只允许 auction_id 加可选的能力声明；声明值非 `1` 一律拒绝，不静默忽略。
    let declared = usize::from(query.contains_key(INDEPENDENT_BUILD_PARAM));
    if query.len() != 1 + declared || (declared == 1 && !declares_independent_build(query)) {
        return Err(());
    }
    parse_http_target(query)
}

/// 连接是否声明理解 hello 的 `independent_build` 字段（旧壳不带该参数）。
pub(super) fn declares_independent_build(query: &BTreeMap<&str, String>) -> bool {
    query.get(INDEPENDENT_BUILD_PARAM).is_some_and(|value| value == "1")
}

/// 旧版只接受无符号规范十进制：`+1`、`01`、空串和超出 i64 的值都必须失败关闭。
pub(super) fn parse_nonnegative(value: Option<&String>, fallback: i64) -> Result<i64, ()> {
    let Some(value) = value else {
        return Ok(fallback);
    };
    let parsed = value.parse::<u64>().map_err(|_| ())?;
    if parsed > i64::MAX as u64 || parsed.to_string() != *value {
        return Err(());
    }
    Ok(parsed as i64)
}

/// 解析旧版规范正十进制页大小，并保留应用端口的相同上限。
pub(super) fn parse_page_size(value: Option<&String>) -> Result<i64, ()> {
    let Some(value) = value else {
        return Ok(DEFAULT_PUBLIC_EVENT_PAGE_SIZE);
    };
    let parsed = value.parse::<u64>().map_err(|_| ())?;
    if parsed == 0 || parsed > MAX_PUBLIC_EVENT_PAGE_SIZE as u64 || parsed.to_string() != *value {
        return Err(());
    }
    Ok(parsed as i64)
}

fn parse_auction_id(value: &str) -> Result<Uuid, ()> {
    parse_public_uuid(value)
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;
    use uuid::Uuid;

    use super::{
        LIST_QUERY_KEYS, WEBSOCKET_QUERY_KEYS, declares_independent_build, parse_http_target,
        parse_websocket_target,
    };
    use crate::query::parse_query;

    const AUCTION_ID: &str = "1f0c0000-0000-4000-8000-000000000002";

    fn list_target(query: &str) -> Result<Uuid, ()> {
        let uri: Uri = format!("/api/v1/events?{query}").parse().expect("合法 URI");
        parse_query(&uri, LIST_QUERY_KEYS).and_then(|query| parse_http_target(&query))
    }

    fn websocket_target(query: &str) -> Result<Uuid, ()> {
        let uri: Uri = format!("/api/v1/ws?{query}").parse().expect("合法 URI");
        parse_query(&uri, WEBSOCKET_QUERY_KEYS).and_then(|query| parse_websocket_target(&query))
    }

    #[test]
    fn targets_resolve_a_single_auction_stream() {
        let auction_id = Uuid::parse_str(AUCTION_ID).expect("固定拍卖标识");
        let list = format!("auction_id={AUCTION_ID}&after=3&page_size=10");
        assert_eq!(list_target(&list), Ok(auction_id));
        assert_eq!(websocket_target(&format!("auction_id={AUCTION_ID}")), Ok(auction_id));
        let declared = format!("auction_id={AUCTION_ID}&independent_build=1");
        assert_eq!(websocket_target(&declared), Ok(auction_id));
    }

    #[test]
    fn independent_build_declaration_is_strict() {
        for value in ["0", "", "true", "11"] {
            let query = format!("auction_id={AUCTION_ID}&independent_build={value}");
            assert_eq!(websocket_target(&query), Err(()), "value={value}");
        }
        let uri: Uri = format!("/api/v1/ws?auction_id={AUCTION_ID}&independent_build=1")
            .parse()
            .expect("合法 URI");
        let declared = parse_query(&uri, WEBSOCKET_QUERY_KEYS).expect("合法查询");
        assert!(declares_independent_build(&declared));
        let uri: Uri = format!("/api/v1/ws?auction_id={AUCTION_ID}").parse().expect("合法 URI");
        let plain = parse_query(&uri, WEBSOCKET_QUERY_KEYS).expect("合法查询");
        assert!(!declares_independent_build(&plain));
    }

    #[test]
    fn scope_and_missing_auction_fail_closed() {
        // `scope`（含 `scope=auctions`）不在白名单内，与其他未知参数走同一拒绝路径。
        let with_scope = format!("auction_id={AUCTION_ID}&scope=auctions");
        for query in ["scope=auctions", with_scope.as_str(), "", "after=0&page_size=1"] {
            assert_eq!(list_target(query), Err(()), "query={query}");
        }
        let with_cursor = format!("auction_id={AUCTION_ID}&after=0");
        for query in ["scope=auctions", with_scope.as_str(), "", with_cursor.as_str()] {
            assert_eq!(websocket_target(query), Err(()), "query={query}");
        }
    }
}
