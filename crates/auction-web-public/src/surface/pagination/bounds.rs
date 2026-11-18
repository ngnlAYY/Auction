//! 编号页发请求前的偏移校验与地址纠偏。

use auction_web_shared::{QueryWriteMode, t};
use dioxus::prelude::*;

use super::location::commit_public_query_pairs;
use crate::surface::{PublicBoardQuery, page_pathname, read_page_query};

/// Public 编号列表的偏移上限，等于上限的偏移仍可请求。
pub(super) const MAX_PUBLIC_PAGE_OFFSET: i64 = 10_000;

pub(super) fn offset_last_page(page_size: i64, max_offset: i64) -> i64 {
    (max_offset / page_size.max(1)).saturating_add(1)
}

/// 请求即将使用的页码是否仍在接口的编号分页范围内。
pub(crate) fn public_page_within_offset(page: i64, page_size: i64) -> bool {
    (1..=offset_last_page(page_size, MAX_PUBLIC_PAGE_OFFSET)).contains(&page)
}

/// 同一页面有多张表时，各表只校正自身拥有的页码。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PageKey {
    Page,
    BidLog,
    Bid,
    Proxy,
    Movement,
}

impl PageKey {
    fn value(self, query: &PublicBoardQuery) -> i64 {
        match self {
            Self::Page | Self::BidLog => query.page,
            Self::Bid => query.bid_page,
            Self::Proxy => query.proxy_page,
            Self::Movement => query.movement_page,
        }
    }

    /// 写入 canonical 页码时同时移除出价记录的旧别名。
    fn keys(self) -> &'static [&'static str] {
        match self {
            Self::Page => &["page"],
            Self::BidLog => &["page", "bidlog_page"],
            Self::Bid => &["bid_page"],
            Self::Proxy => &["proxy_page"],
            Self::Movement => &["movement_page"],
        }
    }

    fn pair(self, page: i64) -> (String, String) {
        (self.keys()[0].to_owned(), page.to_string())
    }

    /// 翻页只写所属页码，保留其他筛选的原文与重复键。
    pub(crate) fn write(
        self,
        state: Signal<PublicBoardQuery>,
        pathname: &str,
        page: i64,
        mode: QueryWriteMode,
    ) -> bool {
        if page_pathname() != pathname {
            return false;
        }
        let pairs: Vec<_> = (page > 1).then(|| self.pair(page)).into_iter().collect();
        commit_public_query_pairs(state, self.keys(), &pairs, mode)
    }
}

type Corrections = Vec<(PageKey, i64)>;

fn corrections(
    query: &PublicBoardQuery,
    limits: impl IntoIterator<Item = (PageKey, i64)>,
) -> Corrections {
    limits
        .into_iter()
        .filter_map(|(key, size)| {
            if key == PageKey::Movement && query.auction_id.is_none() {
                return None;
            }
            let maximum = offset_last_page(size, MAX_PUBLIC_PAGE_OFFSET);
            (key.value(query) > maximum).then_some((key, maximum))
        })
        .collect()
}

fn replace_pages(
    changes: &[(PageKey, i64)],
    expected_path: &str,
    pathname: &str,
    write: impl FnOnce(&[&str], &[(String, String)], QueryWriteMode) -> bool,
) -> Option<bool> {
    if changes.is_empty() || pathname != expected_path {
        return None;
    }
    let keys: Vec<_> = changes.iter().flat_map(|(key, _)| key.keys().iter().copied()).collect();
    let pairs: Vec<_> = changes.iter().map(|(key, page)| key.pair(*page)).collect();
    Some(write(&keys, &pairs, QueryWriteMode::Replace))
}

/// 越界纠偏失败时，重试只重新尝试地址更新。自身已合法的表可以独立加载。
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct PublicPageBounds {
    pending: Memo<Corrections>,
    failed: Signal<Corrections>,
    retry_generation: Signal<u64>,
}

impl PublicPageBounds {
    /// 该页码的地址尚未校正。
    fn blocked(self, key: PageKey) -> bool {
        self.pending.read().iter().any(|(field, _)| *field == key)
    }

    /// 确认本次待校正的页码已被浏览器拒绝。
    pub(crate) fn failed(self, key: PageKey) -> bool {
        *self.pending.read() == *self.failed.read() && self.blocked(key)
    }

    /// 用户明确要求重试后才再次尝试写入。
    pub(crate) fn retry(mut self) {
        let next = self.retry_generation.peek().wrapping_add(1);
        self.retry_generation.set(next);
    }
}

/// 按各接口实际页长计算上限，地址替换成功前，调用方必须暂停该列表的 GET。
pub(crate) fn use_public_page_bounds<L: IntoIterator<Item = (PageKey, i64)>>(
    state: Signal<PublicBoardQuery>,
    pathname: &'static str,
    limits: impl Fn(&PublicBoardQuery) -> L + Copy + 'static,
) -> PublicPageBounds {
    let pending = use_memo(move || {
        let query = state.read();
        corrections(&query, limits(&query))
    });
    let mut failed = use_signal(Vec::new);
    let retry_generation = use_signal(|| 0_u64);
    use_effect(move || {
        let _ = retry_generation();
        // 同一上限下换到不同的越界链接，也要重新检查本次地址。
        drop(state.read());
        let changes = pending();
        let actual_path = page_pathname();
        if changes.is_empty() || actual_path != pathname {
            failed.set(Vec::new());
            return;
        }
        let current = read_page_query();
        if changes != corrections(&current, limits(&current)) {
            return;
        }
        // 替换只提交越界的键。另一张表的页码与尚待修正的筛选必须保留。
        let result = replace_pages(&changes, pathname, &actual_path, |keys, pairs, mode| {
            commit_public_query_pairs(state, keys, pairs, mode)
        });
        failed.set(if result == Some(false) { changes } else { Vec::new() });
    });
    PublicPageBounds { pending, failed, retry_generation }
}

/// 地址拒绝校正时提供原地重试。
#[component]
pub(crate) fn PublicPageClampError(bounds: PublicPageBounds) -> Element {
    rsx! {
        div { class: "ui-pager__error", role: "status",
            span { {t!("pagination-clamp-error")} }
            button { r#type: "button", onclick: move |_| bounds.retry(), {t!("data-retry")} }
        }
    }
}

#[cfg(test)]
mod tests {
    use auction_web_shared::QueryWriteMode;

    use super::{PageKey, corrections, public_page_within_offset, replace_pages};
    use crate::surface::PublicBoardQuery;

    #[test]
    fn account_boundary_replaces_only_the_table_that_exceeds_the_offset() {
        assert!(public_page_within_offset(1001, 10));
        assert!(!public_page_within_offset(1002, 10));
        assert!(!public_page_within_offset(i64::MAX, 10));
        let mut query = PublicBoardQuery { bid_page: 1002, proxy_page: 1001, ..Default::default() };
        let limits = [(PageKey::Bid, 10), (PageKey::Proxy, 10)];
        let changes = corrections(&query, limits);
        assert_eq!(changes, [(PageKey::Bid, 1001)]);
        let replace = |keys: &[&str], pairs: &[(String, String)], mode| {
            assert_eq!(keys, ["bid_page"]);
            assert_eq!(pairs, [("bid_page".to_owned(), "1001".to_owned())]);
            assert_eq!(mode, QueryWriteMode::Replace);
            query.bid_page = 1001;
            true
        };
        assert_eq!(
            replace_pages(&changes, "/me", "/history", |_, _, _| {
                panic!("离开的页面不能改写当前地址")
            }),
            None
        );
        assert_eq!(replace_pages(&changes, "/me", "/me", replace), Some(true));
        assert!(corrections(&query, limits).is_empty());
        assert_eq!(query.proxy_page, 1001);
    }

    #[test]
    fn compensation_splits_bounds_by_each_tables_actual_page_size() {
        let query = PublicBoardQuery {
            page: 51,
            movement_page: 502,
            auction_id: Some("round".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            corrections(&query, [(PageKey::Page, 200), (PageKey::Movement, 20)]),
            [(PageKey::Movement, 501)]
        );
        assert!(public_page_within_offset(51, 200));
        assert!(!public_page_within_offset(502, 20));
        let collapsed = PublicBoardQuery { auction_id: None, ..query };
        assert!(
            corrections(&collapsed, [(PageKey::Page, 200), (PageKey::Movement, 20)]).is_empty()
        );
    }
}
