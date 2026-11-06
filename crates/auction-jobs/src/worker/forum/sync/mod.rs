//! 论坛主题同步 Worker；只编排领取数据库快照 → 渲染 → 远端创建/查找 → 写入发布证明，
//! 远端 POST 无法证明成功时进入人工对账而不重复发帖。
//!
//! `runtime` 持有依赖与预算，`cycle` 负责领取与单项超时边界，`claim` 编排一条主题的
//! 远端流程，`failure` 统一失败码与退避，`links` 校验公开地址。

mod claim;
mod cycle;
mod failure;
mod layout;
mod links;
mod runtime;

pub(crate) use cycle::run_cycle;
pub use runtime::{ForumSyncPorts, ForumSyncRuntime};

pub(super) use links::{resolve_public_url, valid_public_base_url};
