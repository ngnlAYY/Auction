//! PostgreSQL Users/Admin 命令写入的稳定出口。
//!
//! 命令入口按黑名单恢复、创建、用户状态变更和改名回滚分组；每个子模块仍独占自己的
//! 数据库事务流程，调用方只通过这里导出稳定的存储端口实现。

mod blacklist;
mod create;
mod mutation;
mod revert;

pub(crate) use blacklist::ensure_recovery_blacklist_postgres;
pub(crate) use create::create_postgres;
pub(crate) use mutation::mutate_postgres;
pub(crate) use revert::revert_postgres;
