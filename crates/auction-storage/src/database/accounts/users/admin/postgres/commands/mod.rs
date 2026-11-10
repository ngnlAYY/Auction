//! PostgreSQL Users/Admin 命令写入的稳定出口；子模块按黑名单恢复、创建、状态变更和改名回滚分组，
//! 各自独占事务流程，调用方只通过这里导出的稳定端口实现访问。

mod blacklist;
mod create;
mod mutation;
mod revert;

pub(crate) use blacklist::ensure_recovery_blacklist_postgres;
pub(crate) use create::create_postgres;
pub(crate) use mutation::mutate_postgres;
pub(crate) use revert::revert_postgres;
