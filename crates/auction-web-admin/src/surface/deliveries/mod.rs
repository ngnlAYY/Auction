//! Admin delivery outbox 页面装配。
//! 页面状态、筛选器、表格/分页、确认对话框、详情页与展示辅助分别由子模块拥有；外部传输
//! 结果仍由服务端命令端口证明，浏览器只提交安全投影中的 fencing version。

mod detail;
mod dialog;
mod filters;
pub(crate) mod labels;
mod model;
mod page;
mod sent_proof;
mod table;

pub(super) use detail::detail_page;
pub(super) use page::page;
