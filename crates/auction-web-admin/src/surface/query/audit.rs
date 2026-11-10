//! 审计列表的 query 状态：九项筛选、页码与排序。
//! 表单草稿只在提交时写入 URL；文本上限与输入框 maxlength 对齐。

use auction_web_shared::{QueryState, use_query_state};

use crate::api::AdminAuditFilter;

use super::super::sort::SortDirection;
use super::primitives::{
    parse_choice, parse_page, parse_sort_column, parse_sort_direction, parse_text, push_page,
    push_sort, push_text,
};

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

const AUDIT_ACTOR_TYPE_VALUES: &[&str] = &["public_user", "admin", "worker", "system"];
const AUDIT_SOURCE_VALUES: &[&str] = &["public", "admin", "worker", "system", "cli"];
const AUDIT_RESULT_VALUES: &[&str] = &["success", "rejected", "failure"];

/// 审计列表可排序列：发生时间、操作者、动作、实体、来源、结果。
const AUDIT_SORT_COLUMNS: &[&str] = &["at", "actor", "action", "entity", "source", "result"];

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AuditQuery {
    pub filter: AdminAuditFilter,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序（发生时间倒序）。合法值见 `AUDIT_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for AuditQuery {
    fn default() -> Self {
        Self {
            filter: AdminAuditFilter::default(),
            page: 1,
            sort: String::new(),
            direction: SortDirection::Asc,
        }
    }
}

pub(crate) fn use_audit_query_state() -> QueryState<AuditQuery> {
    use_query_state(
        AUDIT_KEYS,
        |pairs| AuditQuery {
            filter: AdminAuditFilter {
                actor: parse_text(pairs, "actor", 256),
                entity: parse_text(pairs, "entity", 385),
                action: parse_text(pairs, "action", 128),
                request_id: parse_text(pairs, "request_id", 128),
                actor_type: parse_choice(pairs, "actor_type", AUDIT_ACTOR_TYPE_VALUES),
                source: parse_choice(pairs, "source", AUDIT_SOURCE_VALUES),
                result: parse_choice(pairs, "result", AUDIT_RESULT_VALUES),
                occurred_from: parse_text(pairs, "from", 32),
                occurred_to: parse_text(pairs, "to", 32),
            },
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, AUDIT_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "actor", &query.filter.actor);
            push_text(&mut pairs, "entity", &query.filter.entity);
            push_text(&mut pairs, "action", &query.filter.action);
            push_text(&mut pairs, "request_id", &query.filter.request_id);
            push_text(&mut pairs, "actor_type", &query.filter.actor_type);
            push_text(&mut pairs, "source", &query.filter.source);
            push_text(&mut pairs, "result", &query.filter.result);
            push_text(&mut pairs, "from", &query.filter.occurred_from);
            push_text(&mut pairs, "to", &query.filter.occurred_to);
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}
