//! PostgreSQL 数据库破坏性重建适配器；根模块只做稳定导出，结果模型、目录预检、破坏性 guard 和
//! SQL quoting 由子模块拥有。调用方必须在进入前完成精确目标确认、备份或一次性环境授权。

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
