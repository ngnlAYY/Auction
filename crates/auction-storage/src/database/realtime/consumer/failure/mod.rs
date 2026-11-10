//! Realtime consumer failure 的事务适配器装配。

mod record;
mod resolve;

pub(super) use record::record_failure_postgres;
pub(crate) use resolve::resolve_failure_postgres_tx;
