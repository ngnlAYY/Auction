//! Admin 结算报表筛选的 URL 状态与浏览器历史同步。
//! 共享查询订阅推进位置版本，页面按版本恢复世界、场次、期号与页码，并复位相关的详情上下文。
//! 目录加载后按服务端事实规范化地址，无效链接保留原文供核对。

use std::{cell::Cell, collections::HashSet, rc::Rc};

use auction_web_shared::query::{location_parts, query_location};
use auction_web_shared::{QueryWriteMode, read_query_pairs, use_query_state, write_query_pairs};
use dioxus::prelude::*;

use crate::api::{MAX_JSON_SAFE_INTEGER, valid_uuid};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SettlementQuery {
    pub world_id: String,
    pub auction_id: String,
    pub sequence: Option<i64>,
    pub page: i64,
}

impl Default for SettlementQuery {
    fn default() -> Self {
        Self { world_id: "all".to_owned(), auction_id: String::new(), sequence: None, page: 1 }
    }
}

/// 本页在地址栏管理的全部 query 键；写回时只替换这些键，其余键原样保留。
const SETTLEMENT_QUERY_KEYS: &[&str] = &["world", "auction", "sequence", "page"];

pub(super) const SETTLEMENT_PAGE_SIZE: &str = "page_size";

/// 显式提供的筛选或场次标识无法解析，不能按缺省查询选中其他场次。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct InvalidSettlementLink;

#[cfg(test)]
pub(super) struct TestSettlementSearch(pub(super) Vec<(String, String)>);

pub(super) fn read_settlement_query() -> Result<SettlementQuery, InvalidSettlementLink> {
    #[cfg(test)]
    if let Some(search) = try_consume_context::<std::rc::Rc<TestSettlementSearch>>() {
        return parse_settlement_pairs(&search.0);
    }
    parse_settlement_pairs(&read_query_pairs())
}

fn parse_settlement_pairs(
    pairs: &[(String, String)],
) -> Result<SettlementQuery, InvalidSettlementLink> {
    let mut query = SettlementQuery::default();
    let mut seen = HashSet::new();
    for (key, value) in pairs {
        if key != SETTLEMENT_PAGE_SIZE && !SETTLEMENT_QUERY_KEYS.contains(&key.as_str()) {
            continue;
        }
        if !seen.insert(key.as_str()) {
            return Err(InvalidSettlementLink);
        }
        match key.as_str() {
            "world" if matches!(value.as_str(), "all" | "main" | "isekai") => {
                query.world_id = value.clone();
            }
            "auction" => {
                if !valid_uuid(value) {
                    return Err(InvalidSettlementLink);
                }
                query.auction_id = value.clone();
            }
            "sequence" => {
                query.sequence = Some(
                    positive_integer(value)
                        .filter(|number| *number <= MAX_JSON_SAFE_INTEGER)
                        .ok_or(InvalidSettlementLink)?,
                );
            }
            "page" => query.page = positive_integer(value).ok_or(InvalidSettlementLink)?,
            // 先拦截重复值，避免分页组件挂载时按首值改写无效地址。单值仍由分页组件解析。
            SETTLEMENT_PAGE_SIZE => {}
            _ => return Err(InvalidSettlementLink),
        }
    }
    // 按期号定位只兼容未提供 UUID 的旧链接；给出 UUID 时必须具备可核对的期号。
    if !query.auction_id.is_empty() && query.sequence.is_none() {
        return Err(InvalidSettlementLink);
    }
    Ok(query)
}

fn positive_integer(value: &str) -> Option<i64> {
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().filter(|number| *number > 0)
}

/// 页面已接纳的位置版本和地址筛选，供分页与回执在执行时复核。
#[derive(Clone, Copy, PartialEq)]
pub(super) struct SettlementLocation {
    pub(super) applied_revision: Signal<u64>,
    revision: Signal<u64>,
    path: Signal<Option<String>>,
    address: Signal<SettlementAddress>,
}

struct SettlementAddress {
    query: SettlementQuery,
    refused_at: Option<String>,
}

impl SettlementLocation {
    /// 共享订阅和筛选恢复之间均可能到达回执，须同时核对浏览器地址与已应用版本。
    pub(super) fn is_current(self) -> bool {
        self.revision.try_peek().is_ok_and(|revision| {
            self.applied_revision.try_peek().is_ok_and(|applied| *revision == *applied)
        }) && self.matches_address()
    }

    /// 核对仍是页面接纳的路径与筛选。各报告的每页数量由各自的分页状态处理。
    pub(super) fn matches_address(self) -> bool {
        self.path.try_peek().is_ok_and(|path| *path == current_path())
            && self.address.try_peek().is_ok_and(|address| {
                read_settlement_query().is_ok_and(|query| query == address.query)
            })
    }

    /// 拒写后页面选择只在本地。若原地址再发生变化，交给外壳保留可能被恢复清除的回执。
    pub(super) fn can_deliver_receipt(self) -> bool {
        self.is_current()
            && self.address.try_peek().is_ok_and(|address| {
                address.refused_at.as_ref().is_none_or(|previous| {
                    current_address().is_some_and(|current| &current == previous)
                })
            })
    }

    /// 页面恢复筛选后才发布已应用的位置版本。
    pub(super) fn apply(mut self, revision: u64, query: SettlementQuery) {
        self.address.set(SettlementAddress { query, refused_at: None });
        self.applied_revision.set(revision);
    }

    /// 将已经应用到页面的筛选写回历史，并同步成功写入的地址基准。
    pub(super) fn write(mut self, query: SettlementQuery, mode: QueryWriteMode) {
        let Some(pairs) = settlement_query_pairs(&query) else { return };
        if write_query_pairs(SETTLEMENT_QUERY_KEYS, &pairs, mode) {
            self.address.set(SettlementAddress { query, refused_at: None });
        } else {
            // 拒写保留地址基准，匹配的目录响应仍可归并到本地筛选。
            let mut address = self.address.write();
            if address.query != query && address.refused_at.is_none() {
                address.refused_at = current_address();
            }
        }
    }
}

/// 记录首读采用的路径与筛选；有效链接视图卸载时销毁这一份基准。
pub(super) fn use_settlement_location(
    revision: Signal<u64>,
    query: &SettlementQuery,
) -> SettlementLocation {
    SettlementLocation {
        applied_revision: use_signal(|| *revision.peek()),
        revision,
        path: use_signal(current_path),
        address: use_signal(|| SettlementAddress { query: query.clone(), refused_at: None }),
    }
}

fn current_path() -> Option<String> {
    current_address().map(|address| location_parts(&address).0.to_owned())
}

fn current_address() -> Option<String> {
    let mut address = if let Some(location) = query_location() {
        location.current()
    } else {
        cfg_select! {
            target_arch = "wasm32" => {
                if let Some(history) = try_consume_context::<Rc<dyn History>>() {
                    history.current_route()
                } else {
                    let location = web_sys::window()?.location();
                    location.pathname().ok()? + &location.search().ok()?
                }
            }
            _ => try_consume_context::<Rc<dyn History>>()?.current_route(),
        }
    };
    if let Some(fragment) = address.find('#') {
        address.truncate(fragment);
    }
    Some(address)
}

/// 序列化规则与旧版一致：默认值键省略；auction 有 ID 时必须同时有 sequence。
fn settlement_query_pairs(query: &SettlementQuery) -> Option<Vec<(String, String)>> {
    if !query.auction_id.is_empty() && query.sequence.is_none() {
        return None;
    }
    let mut pairs = Vec::new();
    if query.world_id != "all" {
        pairs.push(("world".to_owned(), query.world_id.clone()));
    }
    if let Some(sequence) = query.sequence {
        pairs.push(("sequence".to_owned(), sequence.to_string()));
        if !query.auction_id.is_empty() {
            pairs.push(("auction".to_owned(), query.auction_id.clone()));
        }
    }
    if query.page > 1 {
        pairs.push(("page".to_owned(), query.page.to_string()));
    }
    Some(pairs)
}

/// 将共享层认可的同页地址变化记入本页的位置版本。
pub(super) fn use_settlement_location_revision() -> Signal<u64> {
    let next_revision = Cell::new(0_u64);
    // 共享层按实际路径和查询去重，成功的筛选及分页写回也会更新其基准。
    // 即使地址解析出的值相同，新的地址仍须覆盖拒写后保留的本地选择。
    use_query_state(
        &[],
        move |_| next_revision.replace(next_revision.get().saturating_add(1)),
        |_| Vec::new(),
    )
    .state
}

#[cfg(test)]
mod tests {
    use super::{InvalidSettlementLink, SettlementQuery, parse_settlement_pairs};

    const UUID: &str = "12345678-1234-4abc-8000-123456789012";

    fn parse(query: &str) -> Result<SettlementQuery, InvalidSettlementLink> {
        let pairs: Vec<_> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
        parse_settlement_pairs(&pairs)
    }

    #[test]
    fn sequence_only_links_require_an_absent_auction_parameter() {
        let legacy = parse("sequence=7").unwrap();
        assert!(legacy.auction_id.is_empty());
        assert_eq!(legacy.sequence, Some(7));
        let paired = parse(&format!("auction={UUID}&sequence=7&world=main&page=2")).unwrap();
        assert_eq!(paired.auction_id, UUID);
        assert_eq!(paired.sequence, Some(7));
        assert_eq!(paired.world_id, "main");
        assert_eq!(paired.page, 2);
        for invalid in [
            "auction=bad&sequence=7".to_owned(),
            "auction=&sequence=7".to_owned(),
            "auction=00000000-0000-0000-0000-000000000000&sequence=7".to_owned(),
            format!("auction={}&sequence=7", UUID.to_uppercase()),
            "auction=12345678-1234-7abc-8000-123456789012&sequence=7".to_owned(),
            "auction=12345678-1234-4abc-0000-123456789012&sequence=7".to_owned(),
            format!("auction={UUID}"),
        ] {
            assert_eq!(parse(&invalid), Err(InvalidSettlementLink), "{invalid}");
        }
    }

    #[test]
    fn invalid_sequence_and_repeated_parameters_never_select_a_default() {
        assert_eq!(parse(""), Ok(SettlementQuery::default()));
        for invalid in [
            "sequence=",
            "sequence=0",
            "sequence=-1",
            "sequence=7.5",
            "sequence=%2B7",
            "sequence=9007199254740992",
            "sequence=seven",
            "sequence=7&sequence=7",
            "sequence=7&%73equence=8",
            "world=main&world=isekai&sequence=7",
            "world=bad&sequence=7",
            "page=1&page=2",
        ] {
            assert_eq!(parse(invalid), Err(InvalidSettlementLink), "{invalid}");
        }
        assert_eq!(
            parse(&format!("auction={UUID}&auction={UUID}&sequence=7")),
            Err(InvalidSettlementLink)
        );
    }

    #[test]
    fn duplicate_page_size_cannot_select_a_settlement() {
        assert_eq!(parse("sequence=7&page_size=10").unwrap().sequence, Some(7));
        for query in ["sequence=7&page_size=10&page_size=20", "page_size=10&page%5Fsize=10"] {
            assert_eq!(parse(query), Err(InvalidSettlementLink), "{query}");
        }
    }
}
