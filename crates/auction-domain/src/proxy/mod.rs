//! 代理竞价领域模型的稳定出口。
//!
//! 快照模型、授权状态变更和追加式转换校验分别归属相邻模块；本文件只负责装配和公开
//! `auction-domain::proxy` 的兼容导出，避免上层依赖内部文件布局。

mod authorization;
mod model;
mod transition;

pub use model::{
    ProxyAuthorization, ProxyMode, ProxyStatus, ProxyTerminalReason, ProxyTransition,
    ProxyTransitionKind,
};
