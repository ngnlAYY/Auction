//! Auction-BF PostgreSQL v28 的专用备份、校验和接管命令装配；schema/data bridge 仍由
//! `auction-storage` 独占。

mod adoption;
mod backup;
mod common;
mod model;
mod receipt;
mod verification;

pub(super) use adoption::adopt;
pub(super) use backup::backup;
pub(super) use verification::verify_backup_cli;
