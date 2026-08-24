//! Admin delivery outbox 页面装配。
//!
//! 页面状态、筛选器、表格/分页、确认对话框和展示辅助分别由子模块拥有；外部传输结果仍由
//! 服务端命令端口证明，浏览器只提交安全投影中的 fencing version。

mod dialog;
mod filters;
mod helpers;
mod model;
mod page;
mod table;

pub(super) use page::page;
