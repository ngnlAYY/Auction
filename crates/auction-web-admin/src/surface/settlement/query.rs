//! Admin 结算报表筛选的 URL 状态与浏览器历史同步。
//! 不迁到统一的 `use_query_state`：本页 URL 状态拆成四个独立 signal
//! （world/auction/sequence/page），popstate 经 `use_settlement_location_revision` 的 revision
//! 驱动页面 effect 一次性恢复，并同时复位详情上下文（`selection_generation` 防止 recovery
//! 确认的异步回写落到已切换的选择上，见 recovery.rs 提交回调里的同代比较）；目录加载后还要
//! 按服务端事实 canonical 化地址（auction 与 sequence 必须成对、空目录页钳制到末页、解析失败
//! 保留地址显式报错）。这些语义超出通用 hook 的单一状态模型，因此保留本模块的
//! read + push/replace + location revision 三件套，底层地址栏读写与 popstate 监听复用
//! `auction_web_shared::query` 的 `read_query_pairs` / `write_query_pairs` / `use_popstate_revision`。

use std::collections::HashSet;

use auction_web_shared::{
    QueryWriteMode, read_query_pairs, use_popstate_revision, write_query_pairs,
};
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

pub(super) fn replace_settlement_query(query: &SettlementQuery) {
    // 位置已改变时，在旧目录响应写回前再次检查，保留无效链接原文。
    if read_settlement_query().is_err() {
        return;
    }
    write_query_pairs(
        SETTLEMENT_QUERY_KEYS,
        &settlement_query_pairs(query),
        QueryWriteMode::Replace,
    );
}

/// 用户主动切换筛选或分页时创建可返回的浏览器历史项。
pub(super) fn push_settlement_query(query: &SettlementQuery) {
    write_query_pairs(SETTLEMENT_QUERY_KEYS, &settlement_query_pairs(query), QueryWriteMode::Push);
}

/// 序列化规则与旧版一致：默认值键省略，auction 与 sequence 成对写入。
fn settlement_query_pairs(query: &SettlementQuery) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    if query.world_id != "all" {
        pairs.push(("world".to_owned(), query.world_id.clone()));
    }
    if let Some(sequence) = query.sequence {
        pairs.push(("sequence".to_owned(), sequence.to_string()));
    }
    if !query.auction_id.is_empty() {
        pairs.push(("auction".to_owned(), query.auction_id.clone()));
    }
    if query.page > 1 {
        pairs.push(("page".to_owned(), query.page.to_string()));
    }
    pairs
}

/// 监听同一路径的浏览器返回/前进，确保筛选状态不会与地址栏分离。
pub(super) fn use_settlement_location_revision() -> Signal<u64> {
    let revision = use_signal(|| 0_u64);
    use_popstate_revision(revision);
    revision
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
