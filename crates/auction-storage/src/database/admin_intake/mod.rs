//! PostgreSQL Admin 装备录入 owner 的事务入口；事实、审计与 realtime 记录同属一个
//! `PostgresCompetitionTransaction`，由应用层在一个事务内按顺序调用。

use std::future::Future;

use auction_application::{
    AdminIntakeRecord, AdminIntakeResult, AdminIntakeTransaction, AdminIntakeWrite,
    CompetitionTransactionError,
};

use super::competition::PostgresCompetitionTransaction;

mod record;
mod write;

#[allow(clippy::manual_async_fn)]
impl AdminIntakeTransaction for PostgresCompetitionTransaction {
    fn add_admin_intake(
        &mut self,
        write: AdminIntakeWrite,
    ) -> impl Future<Output = Result<AdminIntakeResult, CompetitionTransactionError>> + Send {
        async move { write::add_admin_intake_inner(self, write).await }
    }

    fn append_admin_intake_record(
        &mut self,
        record: AdminIntakeRecord,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { record::append_admin_intake_record_inner(self, record).await }
    }
}

fn failed(message: &str) -> CompetitionTransactionError {
    CompetitionTransactionError::Failed(message.to_owned())
}
