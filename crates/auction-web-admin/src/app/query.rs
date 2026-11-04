//! Admin 列表页筛选/分页的 URL query 状态定义。
//! 每个页面声明自己管理的 query 键、解析（非法值回退默认值）与序列化（等于默认值的键
//! 省略，保持地址栏干净）规则；状态同步统一交给 `auction_web_shared::use_query_state`：
//! 挂载时从地址栏读初值，用户变更经 `push`/`replace` 写回，浏览器前进/后退（popstate）
//! 由 hook 自动恢复状态信号。
//! 入 URL 的只有筛选、分页与排序这类「视图状态」；输入草稿、对话框开合、操作反馈等瞬态仍由
//! 各页面的 signal 持有，不写地址栏。
//! 排序键为 `sort`（列标识）+ `dir`（仅降序时写入 `desc`）：服务端列表端点不支持排序参数，
//! 排序由浏览器对当前页执行，列标识只在本地消费，不转发给 API（见 `sort.rs`）。

use auction_web_shared::{QueryState, use_query_state};

use crate::api::{AdminAuditFilter, AdminMailIntakeQuery, AdminUserStatusFilter};

use super::deliveries::helpers::DELIVERY_STATUS_VALUES;
use super::sort::SortDirection;

/// 取某个键的首个值。
fn pair_value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(pair_key, _)| pair_key == key).map(|(_, value)| value.as_str())
}

/// 解析正整数页码；缺失、零、负数或非法字符串都回退第一页。
fn parse_page(pairs: &[(String, String)], key: &str) -> i64 {
    pair_value(pairs, key)
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|page| *page > 0)
        .unwrap_or(1)
}

/// 页码大于 1 时才写入地址栏。
fn push_page(pairs: &mut Vec<(String, String)>, key: &str, page: i64) {
    if page > 1 {
        pairs.push((key.to_owned(), page.to_string()));
    }
}

/// 解析闭集枚举值；不在允许集合内（含缺省）时回退空串表示「全部」。
fn parse_choice(pairs: &[(String, String)], key: &str, allowed: &[&str]) -> String {
    pair_value(pairs, key).filter(|value| allowed.contains(value)).unwrap_or_default().to_owned()
}

/// 解析自由文本筛选：trim、长度上限（与对应输入框 maxlength 对齐）、剔除控制字符；
/// 不满足任一条件视为未设置，避免把畸形地址栏值直接转发给 API。
fn parse_text(pairs: &[(String, String)], key: &str, maximum: usize) -> String {
    let Some(raw) = pair_value(pairs, key) else {
        return String::new();
    };
    let value = raw.trim();
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        String::new()
    } else {
        value.to_owned()
    }
}

/// 文本筛选非空时才写入地址栏。
fn push_text(pairs: &mut Vec<(String, String)>, key: &str, value: &str) {
    if !value.is_empty() {
        pairs.push((key.to_owned(), value.to_owned()));
    }
}

/// 解析排序列：不在该页允许集合内（含缺省）回退空串，表示保持服务端原始顺序。
fn parse_sort_column(pairs: &[(String, String)], allowed: &[&str]) -> String {
    parse_choice(pairs, "sort", allowed)
}

/// 解析排序方向；只有排序列存在时方向才有意义，非法值回退升序。
fn parse_sort_direction(pairs: &[(String, String)]) -> SortDirection {
    SortDirection::parse(pair_value(pairs, "dir"))
}

/// 解析自动轮询间隔：合法值为 10/30/60 秒；缺省或非法回退 0（关闭）。
fn parse_poll_seconds(pairs: &[(String, String)]) -> u64 {
    pair_value(pairs, "poll")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| super::poll::POLL_INTERVAL_VALUES.contains(seconds))
        .unwrap_or(0)
}

/// 轮询开启（间隔非 0）时才写入地址栏。
fn push_poll_seconds(pairs: &mut Vec<(String, String)>, seconds: u64) {
    if seconds > 0 {
        pairs.push(("poll".to_owned(), seconds.to_string()));
    }
}

/// 排序列非空时才写入地址栏；方向仅降序（非默认）时写入 `dir=desc`。
fn push_sort(pairs: &mut Vec<(String, String)>, sort: &str, direction: SortDirection) {
    if sort.is_empty() {
        return;
    }
    pairs.push(("sort".to_owned(), sort.to_owned()));
    if let Some(direction) = direction.as_query_value() {
        pairs.push(("dir".to_owned(), direction.to_owned()));
    }
}

const AUCTIONS_KEYS: &[&str] = &["world", "page", "sort", "dir"];

/// 拍卖列表可排序列：场次号、状态、开拍时间、结拍时间。
const AUCTIONS_SORT_COLUMNS: &[&str] = &["sequence", "status", "open_at", "end_at"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AuctionsQuery {
    /// 空串表示全部世界；合法值为 `main` / `isekai`。
    pub world: String,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `AUCTIONS_SORT_COLUMNS`。
    pub sort: String,
    /// 排序方向；仅 `sort` 非空时有意义。
    pub direction: SortDirection,
}

impl Default for AuctionsQuery {
    fn default() -> Self {
        Self { world: String::new(), page: 1, sort: String::new(), direction: SortDirection::Asc }
    }
}

pub(super) fn use_auctions_query_state() -> QueryState<AuctionsQuery> {
    use_query_state(
        AUCTIONS_KEYS,
        |pairs| AuctionsQuery {
            world: parse_choice(pairs, "world", &["main", "isekai"]),
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, AUCTIONS_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "world", &query.world);
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}

const PAGE_ONLY_KEYS: &[&str] = &["page", "sort", "dir"];

/// 凭据列表可排序列：标签、世界、类型、状态、上次检查时间。
const CREDENTIALS_SORT_COLUMNS: &[&str] = &["label", "world", "kind", "status", "checked"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PageQuery {
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `CREDENTIALS_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for PageQuery {
    fn default() -> Self {
        Self { page: 1, sort: String::new(), direction: SortDirection::Asc }
    }
}

pub(super) fn use_page_query_state() -> QueryState<PageQuery> {
    use_query_state(
        PAGE_ONLY_KEYS,
        |pairs| PageQuery {
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, CREDENTIALS_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}

const LOTS_KEYS: &[&str] = &["page", "sort", "dir"];

/// lot 列表可排序列：序号、状态、起拍价、当前价。
const LOTS_SORT_COLUMNS: &[&str] = &["code", "status", "start_price", "current_price"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LotsQuery {
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `LOTS_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for LotsQuery {
    fn default() -> Self {
        Self { page: 1, sort: String::new(), direction: SortDirection::Asc }
    }
}

pub(super) fn use_lots_query_state() -> QueryState<LotsQuery> {
    use_query_state(
        LOTS_KEYS,
        |pairs| LotsQuery {
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, LOTS_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}

// 邮箱轮询输入、详情面板开合是瞬态，不入 URL。

const MAIL_KEYS: &[&str] = &["world", "kind", "status", "page", "sort", "dir"];

const MAIL_KIND_VALUES: &[&str] =
    &["equipment", "bid", "delivery", "registration", "rebind", "unknown"];
const MAIL_STATUS_VALUES: &[&str] = &["accepted", "rejected", "failed"];

/// 邮件入站列表可排序列：接收时间、类型、状态、世界。
const MAIL_SORT_COLUMNS: &[&str] = &["received", "kind", "status", "world"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MailQuery {
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
    pub(super) fn to_intake_query(&self) -> AdminMailIntakeQuery {
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

pub(super) fn use_mail_query_state() -> QueryState<MailQuery> {
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

// 表单草稿只在提交时写入 URL；文本上限与输入框 maxlength 对齐。

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
pub(super) struct AuditQuery {
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

pub(super) fn use_audit_query_state() -> QueryState<AuditQuery> {
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

const DELIVERIES_KEYS: &[&str] = &["status", "page", "sort", "dir"];

/// Delivery 列表可排序列：投递 ID、到付金额、状态、尝试次数。
const DELIVERIES_SORT_COLUMNS: &[&str] = &["id", "cod", "status", "attempts"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DeliveriesQuery {
    /// 空串表示全部状态；合法值见 `DELIVERY_STATUS_VALUES`。
    pub status: String,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `DELIVERIES_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for DeliveriesQuery {
    fn default() -> Self {
        Self { status: String::new(), page: 1, sort: String::new(), direction: SortDirection::Asc }
    }
}

pub(super) fn use_deliveries_query_state() -> QueryState<DeliveriesQuery> {
    use_query_state(
        DELIVERIES_KEYS,
        |pairs| DeliveriesQuery {
            status: parse_choice(pairs, "status", &DELIVERY_STATUS_VALUES),
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, DELIVERIES_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "status", &query.status);
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}

// 筛选输入框是提交前草稿，不入 URL；replay 对话框与负载输入是瞬态，不入 URL。

const REALTIME_KEYS: &[&str] = &[
    "event_stream",
    "event_kind",
    "event_page",
    "failure_stream",
    "failure_consumer",
    "failure_code",
    "failure_state",
    "failure_page",
    "lag_page",
    "poll",
];

/// failure 状态下拉只开放这两个可选值（pending/retry 选项在页面上被禁用）。
const REALTIME_FAILURE_STATE_VALUES: &[&str] = &["dead_letter", "completed"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RealtimeQuery {
    pub event_stream: String,
    pub event_kind: String,
    pub event_page: i64,
    pub failure_stream: String,
    pub failure_consumer: String,
    pub failure_code: String,
    /// 空串表示全部状态；合法值见 `REALTIME_FAILURE_STATE_VALUES`。
    pub failure_state: String,
    pub failure_page: i64,
    pub lag_page: i64,
    /// 自动轮询间隔秒数；0 表示关闭（默认）。合法值见 `POLL_INTERVAL_VALUES`。
    pub poll_seconds: u64,
}

/// realtime 三个分页各自独立入 URL，默认值都是第一页。
impl Default for RealtimeQuery {
    fn default() -> Self {
        Self {
            event_stream: String::new(),
            event_kind: String::new(),
            event_page: 1,
            failure_stream: String::new(),
            failure_consumer: String::new(),
            failure_code: String::new(),
            failure_state: String::new(),
            failure_page: 1,
            lag_page: 1,
            poll_seconds: 0,
        }
    }
}

pub(super) fn use_realtime_query_state() -> QueryState<RealtimeQuery> {
    use_query_state(
        REALTIME_KEYS,
        |pairs| RealtimeQuery {
            event_stream: parse_text(pairs, "event_stream", 256),
            event_kind: parse_text(pairs, "event_kind", 128),
            event_page: parse_page(pairs, "event_page"),
            failure_stream: parse_text(pairs, "failure_stream", 256),
            failure_consumer: parse_text(pairs, "failure_consumer", 128),
            failure_code: parse_text(pairs, "failure_code", 64),
            failure_state: parse_choice(pairs, "failure_state", REALTIME_FAILURE_STATE_VALUES),
            failure_page: parse_page(pairs, "failure_page"),
            lag_page: parse_page(pairs, "lag_page"),
            poll_seconds: parse_poll_seconds(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "event_stream", &query.event_stream);
            push_text(&mut pairs, "event_kind", &query.event_kind);
            push_page(&mut pairs, "event_page", query.event_page);
            push_text(&mut pairs, "failure_stream", &query.failure_stream);
            push_text(&mut pairs, "failure_consumer", &query.failure_consumer);
            push_text(&mut pairs, "failure_code", &query.failure_code);
            push_text(&mut pairs, "failure_state", &query.failure_state);
            push_page(&mut pairs, "failure_page", query.failure_page);
            push_page(&mut pairs, "lag_page", query.lag_page);
            push_poll_seconds(&mut pairs, query.poll_seconds);
            pairs
        },
    )
}

// 快照本身是资源瞬态，只有轮询开关入 URL。

const DASHBOARD_KEYS: &[&str] = &["poll"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DashboardQuery {
    pub poll_seconds: u64,
}

pub(super) fn use_dashboard_query_state() -> QueryState<DashboardQuery> {
    use_query_state(
        DASHBOARD_KEYS,
        |pairs| DashboardQuery { poll_seconds: parse_poll_seconds(pairs) },
        |query| {
            let mut pairs = Vec::new();
            push_poll_seconds(&mut pairs, query.poll_seconds);
            pairs
        },
    )
}

const RENAMES_KEYS: &[&str] = &["name", "page", "sort", "dir"];

/// 改名历史可排序列：改名时间、原用户名、新用户名、操作者。
const RENAMES_SORT_COLUMNS: &[&str] = &["created", "previous", "next", "actor"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RenamesQuery {
    pub name: String,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `RENAMES_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for RenamesQuery {
    fn default() -> Self {
        Self { name: String::new(), page: 1, sort: String::new(), direction: SortDirection::Asc }
    }
}

pub(super) fn use_renames_query_state() -> QueryState<RenamesQuery> {
    use_query_state(
        RENAMES_KEYS,
        |pairs| RenamesQuery {
            name: parse_text(pairs, "name", 256),
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, RENAMES_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "name", &query.name);
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}

// 名称输入框是提交前草稿，不入 URL。

const USERS_KEYS: &[&str] = &["name", "status", "page", "sort", "dir"];

/// 用户列表可排序列：用户名、状态、拉黑次数、创建时间。
const USERS_SORT_COLUMNS: &[&str] = &["username", "status", "blacklist_count", "created_at"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UsersQuery {
    pub name: String,
    pub status: AdminUserStatusFilter,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `USERS_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for UsersQuery {
    fn default() -> Self {
        Self {
            name: String::new(),
            status: AdminUserStatusFilter::All,
            page: 1,
            sort: String::new(),
            direction: SortDirection::Asc,
        }
    }
}

/// 状态筛选序列化：`All` 是默认值，不写地址栏。
fn users_status_value(status: AdminUserStatusFilter) -> Option<&'static str> {
    match status {
        AdminUserStatusFilter::All => None,
        AdminUserStatusFilter::Active => Some("active"),
        AdminUserStatusFilter::Blacklisted => Some("blacklisted"),
    }
}

pub(super) fn use_users_query_state() -> QueryState<UsersQuery> {
    use_query_state(
        USERS_KEYS,
        |pairs| UsersQuery {
            name: parse_text(pairs, "name", 256),
            status: match pair_value(pairs, "status") {
                Some("active") => AdminUserStatusFilter::Active,
                Some("blacklisted") => AdminUserStatusFilter::Blacklisted,
                // 非法值与缺省都回退「全部」，与页面下拉的默认选项一致。
                _ => AdminUserStatusFilter::All,
            },
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, USERS_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "name", &query.name);
            if let Some(status) = users_status_value(query.status) {
                pairs.push(("status".to_owned(), status.to_owned()));
            }
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}
