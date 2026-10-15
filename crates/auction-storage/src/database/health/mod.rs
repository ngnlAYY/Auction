//! 进程心跳的安全投影；PostgreSQL 读写与映射由对应子模块持有，过期心跳不得证明就绪。

use auction_observability::{
    HealthError, HealthFuture, Heartbeat, HeartbeatCommand, HeartbeatStore,
};

use super::DatabaseStore;

mod mapping;
#[cfg(feature = "postgres")]
mod postgres;

impl HeartbeatStore for DatabaseStore {
    fn publish(&self, command: HeartbeatCommand) -> HealthFuture<'_, ()> {
        Box::pin(async move {
            if !command.validate() {
                return Err(HealthError::InvalidCommand);
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres::publish_postgres(store, &command).await,
            }
        })
    }

    fn list_live(&self) -> HealthFuture<'_, Vec<Heartbeat>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres::list_postgres(store).await,
            }
        })
    }
}
