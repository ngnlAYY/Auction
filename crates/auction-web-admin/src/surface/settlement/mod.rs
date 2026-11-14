//! Admin 结算与 recovery 页面装配。
//! 上方选择已结算场次的世界/期号，下方读取对应 UUID 的结算与 recovery 权威报告；
//! 所有屏幕均使用单列自然文档流，由页面统一滚动。

mod page;
mod picker;
mod presentation;
mod query;
mod recovery;
mod report;
pub(super) use page::page;
