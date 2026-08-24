//! service_heartbeats 的后端适配器装配。
//!
//! 心跳是可过期的安全投影；具体的 PostgreSQL/SQLite 写入、读取和字段映射由对应子模块持有。

use auction_observability::{
    HealthError, HealthFuture, Heartbeat, HeartbeatCommand, HeartbeatStore,
};

use super::DatabaseStore;

mod mapping;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;

impl HeartbeatStore for DatabaseStore {
    fn publish(&self, command: HeartbeatCommand) -> HealthFuture<'_, ()> {
        Box::pin(async move {
            if !command.validate() {
                return Err(HealthError::InvalidCommand);
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres::publish_postgres(store, &command).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => sqlite::publish_sqlite(store, &command).await,
            }
        })
    }

    fn list_live(&self) -> HealthFuture<'_, Vec<Heartbeat>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres::list_postgres(store).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => sqlite::list_sqlite(store).await,
            }
        })
    }
}
