//! 运行时开关的 PostgreSQL 查询与 Read Committed 事务；不改变业务表。

mod queries;
mod transaction;

use auction_application::runtime_switches::{
    RuntimeSwitchCommand, RuntimeSwitchEvent, RuntimeSwitchFuture, RuntimeSwitchRead,
    RuntimeSwitchStore,
};

use crate::database::DatabaseStore;

impl RuntimeSwitchStore for DatabaseStore {
    fn read(&self) -> RuntimeSwitchFuture<'_, RuntimeSwitchRead> {
        let store = self;
        Box::pin(queries::read(store))
    }

    fn write(&self, command: RuntimeSwitchCommand) -> RuntimeSwitchFuture<'_, RuntimeSwitchEvent> {
        let store = self;
        Box::pin(async move {
            command.validate()?;
            crate::database::backend::retry_conflicts(
                store.pool(),
                auction_observability::DatabaseTransactionOwner::RuntimeSwitch,
                crate::database::backend::SERIALIZABLE_ATTEMPTS,
                |error| {
                    matches!(error, transaction::WriteError::Sql(error)
                    if crate::database::backend::observed_serialization_conflict(error))
                },
                || transaction::write(store, &command),
            )
            .await
            .map_err(transaction::WriteError::public)
        })
    }
}
