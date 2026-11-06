//! 运行时开关的 PostgreSQL 查询与 Read Committed 事务；不改变业务表。

mod queries;
mod transaction;

use super::DatabaseStore;
use auction_application::runtime_switches::{
    RuntimeSwitchCommand, RuntimeSwitchEvent, RuntimeSwitchFuture, RuntimeSwitchRead,
    RuntimeSwitchStore,
};

impl RuntimeSwitchStore for DatabaseStore {
    fn read(&self) -> RuntimeSwitchFuture<'_, RuntimeSwitchRead> {
        let Self::Postgres(store) = self;
        Box::pin(queries::read(store))
    }

    fn write(&self, command: RuntimeSwitchCommand) -> RuntimeSwitchFuture<'_, RuntimeSwitchEvent> {
        let Self::Postgres(store) = self;
        Box::pin(async move {
            command.validate()?;
            super::backend::retry_conflicts(
                store.pool(),
                auction_observability::DatabaseTransactionOwner::RuntimeSwitch,
                super::backend::SERIALIZABLE_ATTEMPTS,
                |error| {
                    matches!(error, transaction::WriteError::Sql(error)
                    if super::backend::observed_serialization_conflict(error))
                },
                || transaction::write(store, &command),
            )
            .await
            .map_err(transaction::WriteError::public)
        })
    }
}
