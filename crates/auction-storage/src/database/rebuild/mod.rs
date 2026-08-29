//! PostgreSQL 数据库破坏性重建适配器。
//!
//! 根模块只负责稳定导出；公开结果模型、目录预检、破坏性 guard 和动态 SQL quoting 分别由
//! 子模块拥有。调用方仍必须在进入本边界前完成精确目标确认、备份或一次性环境授权。

mod catalog;
mod guard;
mod model;
mod sql;

pub(crate) use guard::DATABASE_ADVISORY_LOCK_PREFIX;
pub use guard::PostgresDatabaseRebuildGuard;
pub use model::{
    POSTGRES_REBUILD_PREFLIGHT_FORMAT, PostgresRebuildError, PostgresRebuildPreflight,
    PostgresRebuildResult,
};
