//! W4.6 玩家支持视图的 query 状态：检索入口只有用户与参考编号，二者互斥。

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

pub(crate) fn use_support_query_state() -> QueryState<SupportQuery> {
    use_query_state(
        SUPPORT_KEYS,
        |pairs| SupportQuery {
            user: parse_text(pairs, "user", 256),
            reference: parse_uuid_text(pairs, "reference"),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "user", &query.user);
            push_text(&mut pairs, "reference", &query.reference);
            pairs
        },
    )
}
