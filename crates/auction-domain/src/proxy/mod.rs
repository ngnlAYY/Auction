//! 代理竞价领域模型的稳定出口。

mod authorization;
mod model;
mod transition;

pub use model::{
    ProxyAuthorization, ProxyMode, ProxyStatus, ProxyTerminalReason, ProxyTransition,
    ProxyTransitionKind,
};
