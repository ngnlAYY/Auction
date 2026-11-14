//! 告警台的 query 状态：状态筛选（闭集，默认 `active`）与页码。
//! 备注与静默时长是提交前草稿，不入 URL。

use auction_web_shared::{QueryState, use_query_state};

use super::primitives::{pair_value, parse_page, push_page};

const ALERTS_KEYS: &[&str] = &["status", "page"];

/// 告警台状态筛选闭集；与服务端 `status` 参数一致。
const ALERT_STATUSES: &[&str] = &["active", "resolved", "all"];

/// 缺省筛选只看活动告警；等于缺省值时不写地址栏。
const DEFAULT_ALERT_STATUS: &str = "active";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AlertsQuery {
    /// 合法值见 `ALERT_STATUSES`。
    pub status: &'static str,
    pub page: i64,
}

impl Default for AlertsQuery {
    fn default() -> Self {
        Self { status: DEFAULT_ALERT_STATUS, page: 1 }
    }
}

/// 把输入映射到状态闭集；未知值返回 `None`，由调用方决定回退或忽略。
pub(crate) fn alert_status_choice(value: &str) -> Option<&'static str> {
    ALERT_STATUSES.iter().copied().find(|status| *status == value)
}

/// 非法状态回退 `active`，非法页码回退第一页。
fn parse_alerts_query(pairs: &[(String, String)]) -> AlertsQuery {
    AlertsQuery {
        status: pair_value(pairs, "status")
            .and_then(alert_status_choice)
            .unwrap_or(DEFAULT_ALERT_STATUS),
        page: parse_page(pairs, "page"),
    }
}

fn alerts_query_pairs(query: &AlertsQuery) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    if query.status != DEFAULT_ALERT_STATUS {
        pairs.push(("status".to_owned(), query.status.to_owned()));
    }
    push_page(&mut pairs, "page", query.page);
    pairs
}

pub(crate) fn use_alerts_query_state() -> QueryState<AlertsQuery> {
    use_query_state(ALERTS_KEYS, parse_alerts_query, alerts_query_pairs)
}

#[cfg(test)]
mod tests {
    use super::{AlertsQuery, alert_status_choice, alerts_query_pairs, parse_alerts_query};

    fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect()
    }

    #[test]
    fn missing_or_invalid_values_fall_back_to_active_first_page() {
        assert_eq!(parse_alerts_query(&[]), AlertsQuery::default());
        assert_eq!(
            parse_alerts_query(&pairs(&[("status", "silenced"), ("page", "0")])),
            AlertsQuery { status: "active", page: 1 }
        );
        assert_eq!(
            parse_alerts_query(&pairs(&[("status", "ALL"), ("page", "-3")])),
            AlertsQuery { status: "active", page: 1 }
        );
        assert_eq!(parse_alerts_query(&pairs(&[("page", "abc")])).page, 1);
    }

    #[test]
    fn closed_status_and_positive_page_round_trip() {
        for status in ["active", "resolved", "all"] {
            let query = AlertsQuery { status, page: 3 };
            assert_eq!(parse_alerts_query(&alerts_query_pairs(&query)), query);
        }
        assert_eq!(alert_status_choice("resolved"), Some("resolved"));
        assert_eq!(alert_status_choice(""), None);
    }

    #[test]
    fn default_values_are_omitted_from_the_address_bar() {
        assert!(alerts_query_pairs(&AlertsQuery::default()).is_empty());
        assert_eq!(
            alerts_query_pairs(&AlertsQuery { status: "resolved", page: 2 }),
            pairs(&[("status", "resolved"), ("page", "2")])
        );
        assert_eq!(
            alerts_query_pairs(&AlertsQuery { status: "active", page: 4 }),
            pairs(&[("page", "4")])
        );
    }
}
