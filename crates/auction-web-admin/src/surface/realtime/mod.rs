//! Admin realtime 运维页面的模块装配。
//! 页面、总览指标卡、筛选器、事件元数据、lag 表和 failure 表分别由子模块拥有；replay 仍由页面
//! 状态显式驱动，不在浏览器端重建 realtime 状态机。

mod event;
mod failure;
mod filters;
mod lag;
mod overview;
mod page;

pub(super) use page::outbox_page;
