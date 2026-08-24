//! Auction-BF PostgreSQL v28 的专用备份、校验和接管命令装配。
//!
//! 旧版 golang-migrate 历史不能伪装成普通 Rust SQLx 备份。本目录按备份、证据校验、
//! 接管预检、回执和共享模型拆分；实际 schema/data bridge 仍由 `auction-storage` 独占。

mod adoption;
mod backup;
mod common;
mod model;
mod receipt;
mod verification;

pub(super) use adoption::adopt;
pub(super) use backup::backup;
pub(super) use verification::verify_backup_cli;
