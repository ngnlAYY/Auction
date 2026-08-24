//! Realtime consumer failure 的事务适配器装配。

mod record;
mod resolve;

#[cfg(feature = "postgres")]
pub(super) use record::record_failure_postgres;
#[cfg(feature = "sqlite")]
pub(super) use record::record_failure_sqlite;
#[cfg(feature = "postgres")]
pub(super) use resolve::resolve_failure_postgres;
#[cfg(feature = "sqlite")]
pub(super) use resolve::resolve_failure_sqlite;
