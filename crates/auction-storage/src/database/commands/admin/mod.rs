//! Admin 命令的数据库适配装配；intake、lot、mail 与 runtime 各自拥有独立的写入路径，
//! 事务边界、幂等与审计仍由各子模块在自己的事务内完成。

pub(super) mod intake;
pub(super) mod lot;
pub(super) mod mail;
pub(super) mod runtime;
