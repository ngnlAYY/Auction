//! Public 竞价命令组件的稳定出口。
//! 普通竞价和代理授权拥有独立的交互状态；公共金额提示、错误映射和重试命令模型由
//! `pending` 模块共享，保证命令 id、服务端校验和 i18n 边界不被页面拆分破坏。

mod bid;
mod pending;
mod proxy;
mod retry;

pub(super) use bid::{BidForm, BidFormInput, BidSuccess};
pub(super) use pending::{BidConfirmCommand, ProxyConfirmCommand, PublicCommandIdentity};
pub(super) use proxy::{ProxyFormInput, proxy_form};
pub(super) use retry::BidRetry;
