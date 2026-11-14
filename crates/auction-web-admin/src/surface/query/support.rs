//! 玩家支持视图的 query 状态：检索入口只有用户与参考编号，二者互斥。

use auction_web_shared::{QueryState, use_query_state};

use super::primitives::{parse_text, parse_uuid_text, push_text};

const SUPPORT_KEYS: &[&str] = &["user", "reference"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SupportQuery {
    pub user: String,
    pub reference: String,
}

impl SupportQuery {
    /// 语义校验：恰好一个非空入口；参考编号必须是规范 UUID。
    pub(crate) fn valid(&self) -> bool {
        match (self.user.is_empty(), self.reference.is_empty()) {
            (false, true) => true,
            (true, false) => crate::api::valid_uuid(&self.reference),
            _ => false,
        }
    }
}

/// 地址栏只恢复格式合法的入口；是否恰好一个入口由 [`SupportQuery::valid`] 在请求前判定。
fn parse_support_query(pairs: &[(String, String)]) -> SupportQuery {
    SupportQuery {
        user: parse_text(pairs, "user", 256),
        reference: parse_uuid_text(pairs, "reference"),
    }
}

fn support_query_pairs(query: &SupportQuery) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    push_text(&mut pairs, "user", &query.user);
    push_text(&mut pairs, "reference", &query.reference);
    pairs
}

pub(crate) fn use_support_query_state() -> QueryState<SupportQuery> {
    use_query_state(SUPPORT_KEYS, parse_support_query, support_query_pairs)
}

#[cfg(test)]
mod tests {
    use super::{SupportQuery, parse_support_query, support_query_pairs};

    const REFERENCE: &str = "0b5f1c2e-8a7d-4c3b-9e1f-2a3b4c5d6e7f";

    fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect()
    }

    fn query(user: &str, reference: &str) -> SupportQuery {
        SupportQuery { user: user.to_owned(), reference: reference.to_owned() }
    }

    #[test]
    fn exactly_one_entry_round_trips_through_the_address_bar() {
        for applied in [query("alice", ""), query("", REFERENCE)] {
            assert!(applied.valid());
            assert_eq!(parse_support_query(&support_query_pairs(&applied)), applied);
        }
        assert!(support_query_pairs(&SupportQuery::default()).is_empty());
    }

    #[test]
    fn malformed_reference_and_blank_user_are_treated_as_unset() {
        let parsed = parse_support_query(&pairs(&[("user", "   "), ("reference", "REF-1")]));
        assert_eq!(parsed, SupportQuery::default());
        assert!(!parsed.valid());
        assert_eq!(parse_support_query(&pairs(&[("user", " bob ")])), query("bob", ""));
    }

    #[test]
    fn both_or_neither_entries_are_rejected_before_requesting() {
        assert!(!query("alice", REFERENCE).valid());
        assert!(!SupportQuery::default().valid());
        assert!(!query("", "not-a-uuid").valid());
    }
}
