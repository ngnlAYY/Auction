//! 审计列表的 query 状态：九项筛选、页码与排序。
//! 表单草稿只在提交时写入 URL，无效历史值保留供操作员更正。

use auction_web_shared::{QueryState, use_query_state};

use super::super::audit::AuditFilterState;
#[cfg(test)]
use super::super::realtime::use_test_history;
use super::super::sort::SortDirection;
use super::primitives::{
    parse_page, parse_sort_column, parse_sort_direction, push_page, push_sort,
};
use crate::api::AdminAuditFilter;

const AUDIT_KEYS: &[&str] = &[
    "actor",
    "entity",
    "action",
    "request_id",
    "actor_type",
    "source",
    "result",
    "from",
    "to",
    "page",
    "sort",
    "dir",
];

/// 审计列表可排序列：发生时间、操作者、动作、实体、来源、结果。
const AUDIT_SORT_COLUMNS: &[&str] = &["at", "actor", "action", "entity", "source", "result"];

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AuditQuery {
    pub filter: AuditFilterState,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序（发生时间倒序）。合法值见 `AUDIT_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for AuditQuery {
    fn default() -> Self {
        Self {
            filter: AuditFilterState::default(),
            page: 1,
            sort: String::new(),
            direction: SortDirection::Asc,
        }
    }
}

impl AuditQuery {
    /// 提交筛选从第一页开始，并沿用当前排序。
    pub(crate) fn with_filter(self, filter: AdminAuditFilter) -> Self {
        Self { filter: filter.into(), page: 1, ..self }
    }

    /// 无效地址等待更正，不能按缺省筛选读取审计记录。
    pub(crate) fn page_request(&self) -> Option<(i64, AdminAuditFilter)> {
        Some((self.page, self.filter.request()?))
    }

    #[cfg(test)]
    pub(crate) fn restored_from_history(&self) -> Self {
        parse_audit_query(&audit_query_pairs(self))
    }
}

pub(crate) fn use_audit_query_state() -> QueryState<AuditQuery> {
    let location = use_query_state(AUDIT_KEYS, parse_audit_query, audit_query_pairs);
    #[cfg(test)]
    use_test_history(location.state, parse_audit_query, audit_query_pairs);
    location
}

fn parse_audit_query(pairs: &[(String, String)]) -> AuditQuery {
    AuditQuery {
        filter: AuditFilterState::restore(pairs),
        page: parse_page(pairs, "page"),
        sort: parse_sort_column(pairs, AUDIT_SORT_COLUMNS),
        direction: parse_sort_direction(pairs),
    }
}

fn audit_query_pairs(query: &AuditQuery) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    query.filter.append_pairs(&mut pairs);
    push_page(&mut pairs, "page", query.page);
    push_sort(&mut pairs, &query.sort, query.direction);
    pairs
}

#[cfg(test)]
mod tests {
    use auction_time::Timestamp;

    use super::{AuditQuery, audit_query_pairs, parse_audit_query};
    use crate::api::AdminAuditFilter;

    #[test]
    fn submitted_and_restored_text_filters_share_one_url_identity() {
        let applied = AuditQuery::default().with_filter(AdminAuditFilter {
            actor: " alice ".to_owned(),
            entity: " auction:sample lot ".to_owned(),
            action: " auction.create ".to_owned(),
            request_id: "  ".to_owned(),
            ..AdminAuditFilter::default()
        });
        let values = [
            ("actor".to_owned(), "alice".to_owned()),
            ("entity".to_owned(), "auction:sample lot".to_owned()),
            ("action".to_owned(), "auction.create".to_owned()),
        ];
        assert_eq!(audit_query_pairs(&applied), values);
        assert_eq!(parse_audit_query(&values), applied);
        let invalid = [("actor".to_owned(), " invalid\0actor ".to_owned())];
        let restored = parse_audit_query(&invalid);
        assert_eq!(audit_query_pairs(&restored), invalid);
        assert!(restored.page_request().is_none());
    }

    #[test]
    fn legacy_local_audit_link_is_rewritten_with_an_absolute_time() {
        let query = parse_audit_query(&[("from".to_owned(), "2026-11-18T09:05".to_owned())]);
        let (_, filter) = query.page_request().unwrap();
        assert_eq!(filter.occurred_from, "2026-11-18T09:05:00Z");
        assert_eq!(audit_query_pairs(&query), [("from".to_owned(), filter.occurred_from)]);
    }

    #[test]
    fn offset_audit_link_round_trips_without_changing_the_instant() {
        let input = [("to".to_owned(), "2026-11-18T09:05:37.500123456+08:00".to_owned())];
        let query = parse_audit_query(&input);
        let serialized = audit_query_pairs(&query);
        assert_eq!(serialized, [("to".to_owned(), "2026-11-18T01:05:37.500123456Z".to_owned())]);
        assert_eq!(parse_audit_query(&serialized), query);
        assert_eq!(
            Timestamp::parse_from_rfc3339(&query.page_request().unwrap().1.occurred_to).unwrap(),
            Timestamp::parse_from_rfc3339(&input[0].1).unwrap()
        );
    }

    #[test]
    fn malformed_time_does_not_become_an_unbounded_filter() {
        let invalid = [("from".to_owned(), "2026-11-31T09:05:00+08:00".to_owned())];
        let query = parse_audit_query(&invalid);
        assert_eq!(audit_query_pairs(&query), invalid);
        assert!(query.page_request().is_none());
    }

    #[test]
    fn audit_recovery_keeps_repeated_keys_and_control_characters() {
        for (key, valid) in [
            ("actor", "alice"),
            ("entity", "auction:sample"),
            ("action", "auction.create"),
            ("request_id", "request-1"),
            ("actor_type", "admin"),
            ("source", "admin"),
            ("result", "success"),
            ("from", "2026-11-18T09:00:00Z"),
            ("to", "2026-11-18T10:00:00Z"),
        ] {
            let pairs = [(key.to_owned(), valid.to_owned())];
            assert!(parse_audit_query(&pairs).page_request().is_some());
            for pairs in [
                vec![(key.to_owned(), String::new()), (key.to_owned(), valid.to_owned())],
                vec![(key.to_owned(), format!("{valid}\n"))],
            ] {
                let query = parse_audit_query(&pairs);
                assert_eq!(audit_query_pairs(&query), pairs);
                assert!(query.page_request().is_none(), "{key}");
            }
        }
    }
}
