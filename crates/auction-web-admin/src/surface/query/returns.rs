//! 结算退件列表的 query 状态：范围（缺省只看未结案）、状态、证据分类、义务种类、世界与页码。
//! 取值不在契约闭集内时回退为“不限”，不把畸形文本转发给 API。

use auction_contracts::{
    ADMIN_SETTLEMENT_RETURN_CLOSED_STATUSES, ADMIN_SETTLEMENT_RETURN_MATCH_STATES,
    ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS, ADMIN_SETTLEMENT_RETURN_STATUSES,
};
use auction_web_shared::{QueryState, use_query_state};

use super::primitives::{pair_value, parse_choice, parse_page, push_page, push_text};
use crate::api::SettlementReturnFilters;

const RETURNS_KEYS: &[&str] =
    &["scope", "status", "match_state", "obligation_kind", "world", "page"];

/// 世界筛选闭集。
const WORLDS: &[&str] = &["main", "isekai"];

/// 列表视图状态；空串表示该筛选不限。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReturnsQuery {
    /// 同时列出已结案案件；缺省只看未结案。
    pub all: bool,
    pub status: String,
    pub match_state: String,
    pub obligation_kind: String,
    pub world: String,
    pub page: i64,
}

impl Default for ReturnsQuery {
    fn default() -> Self {
        Self {
            all: false,
            status: String::new(),
            match_state: String::new(),
            obligation_kind: String::new(),
            world: String::new(),
            page: 1,
        }
    }
}

impl ReturnsQuery {
    /// 已结案状态不可能是未结案行：范围固定为全部，避免与缺省 open 组成必然空结果。
    pub(crate) fn normalize_scope(&mut self) {
        if ADMIN_SETTLEMENT_RETURN_CLOSED_STATUSES.contains(&self.status.as_str()) {
            self.all = true;
        }
    }

    /// 转为 API 筛选；页码另行传入。
    pub(crate) fn filters(&self) -> SettlementReturnFilters {
        SettlementReturnFilters {
            open_only: !self.all
                && !ADMIN_SETTLEMENT_RETURN_CLOSED_STATUSES.contains(&self.status.as_str()),
            status: self.status.clone(),
            match_state: self.match_state.clone(),
            obligation_kind: self.obligation_kind.clone(),
            world: self.world.clone(),
        }
    }
}

fn parse_returns_query(pairs: &[(String, String)]) -> ReturnsQuery {
    let mut query = ReturnsQuery {
        all: pair_value(pairs, "scope") == Some("all"),
        status: parse_choice(pairs, "status", &ADMIN_SETTLEMENT_RETURN_STATUSES),
        match_state: parse_choice(pairs, "match_state", &ADMIN_SETTLEMENT_RETURN_MATCH_STATES),
        obligation_kind: parse_choice(
            pairs,
            "obligation_kind",
            &ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS,
        ),
        world: parse_choice(pairs, "world", WORLDS),
        page: parse_page(pairs, "page"),
    };
    query.normalize_scope();
    query
}

fn returns_query_pairs(query: &ReturnsQuery) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    if query.all {
        pairs.push(("scope".to_owned(), "all".to_owned()));
    }
    push_text(&mut pairs, "status", &query.status);
    push_text(&mut pairs, "match_state", &query.match_state);
    push_text(&mut pairs, "obligation_kind", &query.obligation_kind);
    push_text(&mut pairs, "world", &query.world);
    push_page(&mut pairs, "page", query.page);
    pairs
}

pub(crate) fn use_returns_query_state() -> QueryState<ReturnsQuery> {
    use_query_state(RETURNS_KEYS, parse_returns_query, returns_query_pairs)
}

#[cfg(test)]
mod tests {
    use super::{ReturnsQuery, parse_returns_query, returns_query_pairs};

    fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect()
    }

    /// 缺省只看未结案第一页；非法取值回退为不限，不转发给 API。
    #[test]
    fn invalid_values_fall_back_to_open_unfiltered_first_page() {
        assert_eq!(parse_returns_query(&[]), ReturnsQuery::default());
        assert!(parse_returns_query(&[]).filters().open_only);
        let parsed = parse_returns_query(&pairs(&[
            ("scope", "everything"),
            ("status", "open"),
            ("match_state", "bound"),
            ("obligation_kind", "payout"),
            ("world", "third"),
            ("page", "-2"),
        ]));
        assert_eq!(parsed, ReturnsQuery::default());
    }

    /// 合法取值往返；等于缺省值的键不写地址栏。
    #[test]
    fn closed_values_round_trip_and_defaults_are_omitted() {
        let query = ReturnsQuery {
            all: true,
            status: "take_unknown".to_owned(),
            match_state: "unbound".to_owned(),
            obligation_kind: "buyer_refund".to_owned(),
            world: "isekai".to_owned(),
            page: 3,
        };
        assert_eq!(parse_returns_query(&returns_query_pairs(&query)), query);
        assert!(!query.filters().open_only);
        assert!(returns_query_pairs(&ReturnsQuery::default()).is_empty());
    }
}
