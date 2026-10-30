//! Realtime consumer failure 的事务适配器装配。

mod record;
mod resolve;

#[cfg(feature = "postgres")]
pub(super) use record::record_failure_postgres;
#[cfg(feature = "postgres")]
pub(crate) use resolve::resolve_failure_postgres_tx;
