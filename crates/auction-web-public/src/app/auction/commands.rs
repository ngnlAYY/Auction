//! Public 竞价命令组件的稳定出口。
//!
//! 普通竞价和代理授权拥有独立的交互状态；公共金额提示、错误映射和重试命令模型由
//! `common.rs` 共享，保证命令 id、服务端校验和 i18n 边界不被页面拆分破坏。

#[path = "commands/bid.rs"]
mod bid;
#[path = "commands/common.rs"]
mod common;
#[path = "commands/proxy.rs"]
mod proxy;

pub(super) use bid::{BidFormInput, bid_form};
pub(super) use proxy::{ProxyFormInput, proxy_form};
