//! Unix 私有目录、跨进程锁和无覆盖发布；所有阻塞操作由归档层调度。
//!
//! 按职责拆分为路径规范化、私有文件校验、目录锁、记录清单、保留清理和原子发布；
//! 归档层只经本模块重导出的入口访问平台实现。

mod lock;
mod path;
mod private;
mod records;
mod retention;
mod store;

pub(super) use retention::{purge_ordinary_expired, retention_snapshot};
pub(super) use store::{persist, read, write_private_export};
