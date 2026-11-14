//! Admin 规则文档页面装配。
//! 页面加载、编辑 workspace、预览、发布状态模型和字段辅助分别由子模块拥有；发布仍要求
//! 最近一次服务端校验结果与当前 revision 同时成立。

mod draft;
mod model;
mod page;
mod preview;
mod workspace;

pub(super) use model::use_rules_draft;
pub(super) use page::page;
