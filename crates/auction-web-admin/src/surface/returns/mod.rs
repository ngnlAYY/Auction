//! Admin 结算退件页面装配：列表、详情、处理动作与闭集译文映射。
//!
//! 页面只消费 `auction-contracts` 的安全投影，不在浏览器重建案件状态机：可用动作与阻断原因由
//! 服务端给出，命令携带 `command_id` 与案件版本，结果未知时只能原样重试同一命令。

mod actions;
mod detail;
mod labels;
mod list;

pub(super) use detail::detail_page;
pub(super) use labels::settlement_return_case;
pub(super) use list::page;
