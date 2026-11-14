//! 邮件入站列表的 query 状态：世界、类型、状态筛选，页码与排序。
//! 邮箱轮询输入、详情面板开合是瞬态，不入 URL。

use auction_web_shared::{QueryState, use_query_state};

use super::super::sort::SortDirection;
use super::primitives::{
    parse_choice, parse_page, parse_sort_column, parse_sort_direction, push_page, push_sort,
    push_text,
};
use crate::api::AdminMailIntakeQuery;

const MAIL_KEYS: &[&str] = &["world", "kind", "status", "page", "sort", "dir"];

const MAIL_KIND_VALUES: &[&str] =
    &["equipment", "bid", "delivery", "registration", "rebind", "unknown"];
const MAIL_STATUS_VALUES: &[&str] = &["accepted", "rejected", "failed"];

/// 邮件入站列表可排序列：接收时间、类型、状态、世界。
const MAIL_SORT_COLUMNS: &[&str] = &["received", "kind", "status", "world"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MailQuery {
    pub world: String,
    /// 空串表示全部类型；合法值见 `MAIL_KIND_VALUES`。
    pub kind: String,
    /// 空串表示全部状态；合法值见 `MAIL_STATUS_VALUES`。
    pub status: String,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `MAIL_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for MailQuery {
    fn default() -> Self {
        Self {
            world: String::new(),
            kind: String::new(),
            status: String::new(),
            page: 1,
            sort: String::new(),
            direction: SortDirection::Asc,
        }
    }
}

impl MailQuery {
    /// 映射为入站列表的 API 查询；URL 不持有的维度保持旧版的空串缺省。
    pub(crate) fn to_intake_query(&self) -> AdminMailIntakeQuery {
        AdminMailIntakeQuery {
            mailbox_key: String::new(),
            world_id: self.world.clone(),
            kind: self.kind.clone(),
            status: self.status.clone(),
            decision: String::new(),
            received_from: String::new(),
            received_to: String::new(),
        }
    }
}

pub(crate) fn use_mail_query_state() -> QueryState<MailQuery> {
    use_query_state(
        MAIL_KEYS,
        |pairs| MailQuery {
            world: parse_choice(pairs, "world", &["main", "isekai"]),
            kind: parse_choice(pairs, "kind", MAIL_KIND_VALUES),
            status: parse_choice(pairs, "status", MAIL_STATUS_VALUES),
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, MAIL_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "world", &query.world);
            push_text(&mut pairs, "kind", &query.kind);
            push_text(&mut pairs, "status", &query.status);
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}
