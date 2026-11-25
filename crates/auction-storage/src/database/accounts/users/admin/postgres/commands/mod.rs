//! PostgreSQL Users/Admin 命令写入出口；黑名单恢复、创建、状态变更和改名回滚各由子模块
//! 独占事务流程；解禁另在买家用户锁下接上寄售周期边界（`unblacklist`）。

mod blacklist;
mod create;
mod mutation;
mod revert;
mod unblacklist;

pub(crate) use blacklist::ensure_recovery_blacklist_postgres;
pub(crate) use create::create_postgres;
pub(crate) use mutation::mutate_postgres;
pub(crate) use revert::revert_postgres;
