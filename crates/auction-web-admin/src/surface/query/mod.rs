//! Admin 列表页筛选/分页的 URL query 状态定义。
//! 每个页面声明自己管理的 query 键、解析（非法值回退默认值）与序列化（等于默认值的键
//! 省略，保持地址栏干净）规则；状态同步统一交给 `auction_web_shared::use_query_state`：
//! 挂载时从地址栏读初值，用户变更经 `push`/`replace` 写回，浏览器前进/后退（popstate）
//! 由 hook 自动恢复状态信号。
//! 入 URL 的只有筛选、分页与排序这类「视图状态」；输入草稿、对话框开合、操作反馈等瞬态仍由
//! 各页面的 signal 持有，不写地址栏。
//! 排序键为 `sort`（列标识）+ `dir`（仅降序时写入 `desc`）：服务端列表端点不支持排序参数，
//! 排序由浏览器对当前页执行，列标识只在本地消费，不转发给 API（见 `sort.rs`）。
//!
//! 子模块按页面拆分：`primitives` 放各页共用的解析/序列化原语，其余模块各自声明本页的
//! query 键、状态结构与 `use_*_query_state` hook；本模块只做聚合与重导出。

mod auctions;
mod audit;
mod credentials;
mod dashboard;
mod deliveries;
mod lots;
mod mail;
mod primitives;
mod realtime;
mod renames;
mod support;
mod users;

pub(super) use auctions::{AuctionsQuery, use_auctions_query_state};
pub(super) use audit::{AuditQuery, use_audit_query_state};
pub(super) use credentials::use_page_query_state;
pub(super) use dashboard::use_dashboard_query_state;
pub(super) use deliveries::use_deliveries_query_state;
pub(super) use lots::use_lots_query_state;
pub(super) use mail::{MailQuery, use_mail_query_state};
pub(super) use realtime::use_realtime_query_state;
pub(super) use renames::use_renames_query_state;
pub(super) use support::{SupportQuery, use_support_query_state};
pub(super) use users::use_users_query_state;
