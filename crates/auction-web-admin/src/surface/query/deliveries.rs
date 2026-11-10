//! Delivery 列表的 query 状态：收件人/引用/模板/类别/世界/时间筛选，页码与排序。
//! 筛选输入框是提交前草稿，不入 URL；replay 对话框与负载输入是瞬态，不入 URL。

use auction_web_shared::{QueryState, use_query_state};

use super::super::deliveries::helpers::DELIVERY_STATUS_VALUES;
use super::super::sort::SortDirection;
use super::primitives::{
    parse_choice, parse_page, parse_sort_column, parse_sort_direction, parse_text, parse_uuid_text,
    push_page, push_sort, push_text,
};

const DELIVERIES_KEYS: &[&str] = &[
    "status",
    "recipient",
    "reference",
    "template",
    "category",
    "world",
    "from",
    "to",
    "page",
    "sort",
    "dir",
];

const DELIVERY_CATEGORY_VALUES: &[&str] =
    &["outbid", "account", "intake", "settlement", "equipment", "operations", "manual"];

/// Delivery 列表可排序列：投递 ID、模板、类别、到付金额、状态、尝试次数。
const DELIVERIES_SORT_COLUMNS: &[&str] =
    &["id", "template", "category", "cod", "status", "attempts"];

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct DeliveriesQuery {
    /// 空串表示全部状态；合法值见 `DELIVERY_STATUS_VALUES`。
    pub status: String,
    /// 收件人精确匹配（大小写不敏感）；空串表示不筛选。
    pub recipient: String,
    /// 投递、义务或通知 ID；非规范 UUID 视为未设置。
    pub reference: String,
    /// 冻结模板键；仅允许模板键字符。
    pub template: String,
    /// 发送类别闭集；空串表示全部。
    pub category: String,
    /// `main` / `isekai`；空串表示全部。
    pub world: String,
    /// 创建时间 RFC3339；空串表示不设界。
    pub created_from: String,
    pub created_to: String,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `DELIVERIES_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl DeliveriesQuery {
    /// 映射为 API 查询结构；`status` 由页面单独传入。
    pub(crate) fn filters(&self) -> crate::api::DeliverySearchFilters {
        crate::api::DeliverySearchFilters {
            recipient: self.recipient.clone(),
            reference: self.reference.clone(),
            template: self.template.clone(),
            category: self.category.clone(),
            world: self.world.clone(),
            created_from: self.created_from.clone(),
            created_to: self.created_to.clone(),
        }
    }
}

/// 模板键只允许小写字母、数字、下划线与连字符，最长 64 字符。
fn parse_template_text(pairs: &[(String, String)], key: &str) -> String {
    let value = parse_text(pairs, key, 64);
    let valid = value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
    });
    if valid { value } else { String::new() }
}

/// 只接受可解析的 RFC3339；非法值按未设置处理。
fn parse_timestamp_text(pairs: &[(String, String)], key: &str) -> String {
    let value = parse_text(pairs, key, 40);
    if chrono::DateTime::parse_from_rfc3339(&value).is_ok() { value } else { String::new() }
}

pub(crate) fn use_deliveries_query_state() -> QueryState<DeliveriesQuery> {
    use_query_state(
        DELIVERIES_KEYS,
        |pairs| DeliveriesQuery {
            status: parse_choice(pairs, "status", &DELIVERY_STATUS_VALUES),
            recipient: parse_text(pairs, "recipient", 256),
            reference: parse_uuid_text(pairs, "reference"),
            template: parse_template_text(pairs, "template"),
            category: parse_choice(pairs, "category", DELIVERY_CATEGORY_VALUES),
            world: parse_choice(pairs, "world", &["main", "isekai"]),
            created_from: parse_timestamp_text(pairs, "from"),
            created_to: parse_timestamp_text(pairs, "to"),
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, DELIVERIES_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "status", &query.status);
            push_text(&mut pairs, "recipient", &query.recipient);
            push_text(&mut pairs, "reference", &query.reference);
            push_text(&mut pairs, "template", &query.template);
            push_text(&mut pairs, "category", &query.category);
            push_text(&mut pairs, "world", &query.world);
            push_text(&mut pairs, "from", &query.created_from);
            push_text(&mut pairs, "to", &query.created_to);
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}
