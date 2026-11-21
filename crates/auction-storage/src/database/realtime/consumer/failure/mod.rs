//! Realtime consumer failure 的事务适配器装配。

pub(super) mod record;
mod resolve;

pub(crate) use resolve::resolve_failure_postgres_tx;
