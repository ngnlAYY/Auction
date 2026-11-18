//! 读写分页与读取页拥有的查询键，保留其他参数原文。

use auction_web_shared::query::write_query_pairs_preserving_segments;
use auction_web_shared::{QueryWriteMode, read_query_pairs};
use dioxus::prelude::*;

#[cfg(test)]
use super::test_support::PageBrowserFixture;
#[cfg(not(test))]
use crate::surface::use_public_query_state;
use crate::surface::{
    PublicBoardQuery, PublicLocationRevision, current_public_pathname, read_public_query,
};

/// 读取页与分页控件使用的当前路由路径。
pub(crate) fn page_pathname() -> String {
    #[cfg(test)]
    if let Some(browser) = try_consume_context::<PageBrowserFixture>() {
        return browser.pathname().to_owned();
    }
    current_public_pathname()
}

/// 从当前地址读取已应用查询。
pub(crate) fn read_page_query() -> PublicBoardQuery {
    #[cfg(test)]
    if let Some(browser) = try_consume_context::<PageBrowserFixture>() {
        return browser.query();
    }
    read_public_query()
}

/// 使用既有的地址状态同步入口挂载页面查询。
#[cfg(not(test))]
pub(crate) fn use_page_query() -> Signal<PublicBoardQuery> {
    use_public_query_state().state
}

/// 宿主页面回归可用地址替身，未提供替身时仍跟踪路由。
#[cfg(test)]
pub(crate) fn use_page_query() -> Signal<PublicBoardQuery> {
    let mut state = use_signal(read_page_query);
    let browser = try_consume_context::<PageBrowserFixture>();
    if let Some(browser) = &browser {
        browser.observe(state);
    }
    let location_revision = try_consume_context::<PublicLocationRevision>();
    use_effect(move || {
        if browser.is_none() {
            track_public_location(location_revision);
            let query = read_page_query();
            if *state.peek() != query {
                state.set(query);
            }
        }
    });
    state
}

/// 在普通查询模型省略值之前读取原始键值对。
pub(crate) fn read_page_pairs() -> Vec<(String, String)> {
    #[cfg(test)]
    if let Some(browser) = try_consume_context::<PageBrowserFixture>() {
        return browser.pairs();
    }
    read_query_pairs()
}

/// 委托现有浏览器入口替换本页拥有的键。
pub(crate) fn write_page_pairs(
    keys: &[&str],
    pairs: &[(String, String)],
    mode: QueryWriteMode,
) -> bool {
    #[cfg(test)]
    if let Some(browser) = try_consume_context::<PageBrowserFixture>() {
        return browser.write(keys, pairs, mode);
    }
    write_query_pairs_preserving_segments(keys, pairs, mode)
}

/// 读取某个键的全部值，区别缺省、空值与重复值。
pub(crate) fn query_values<'a>(
    pairs: &'a [(String, String)],
    key: &str,
) -> impl Iterator<Item = &'a str> {
    pairs.iter().filter(move |(name, _)| name == key).map(|(_, value)| value.as_str())
}

/// 跟踪原始查询对，包括普通查询状态不能表达的重复或非法筛选。
pub(crate) fn use_public_query_pairs(
    state: Signal<PublicBoardQuery>,
) -> Memo<Vec<(String, String)>> {
    #[cfg(test)]
    let browser = try_consume_context::<PageBrowserFixture>();
    let location_revision = try_consume_context::<PublicLocationRevision>();
    use_memo(move || {
        let _ = state.read();
        #[cfg(test)]
        if let Some(browser) = &browser {
            return browser.pairs();
        }
        track_public_location(location_revision);
        read_query_pairs()
    })
}

fn track_public_location(location_revision: Option<PublicLocationRevision>) {
    let _ = dioxus::prelude::router().full_route_string();
    if let Some(PublicLocationRevision(revision)) = location_revision {
        let _ = revision();
    }
}

/// 成功写入本页拥有的键才同步内存查询，避免让地址拒绝的操作触发请求。
pub(crate) fn commit_public_query_pairs(
    mut state: Signal<PublicBoardQuery>,
    keys: &[&str],
    pairs: &[(String, String)],
    mode: QueryWriteMode,
) -> bool {
    if !write_page_pairs(keys, pairs, mode) {
        return false;
    }
    state.set(read_page_query());
    true
}
