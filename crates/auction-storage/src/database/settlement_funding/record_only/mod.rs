//! 仅账目初始化的SQL owner：真实身份、责任快照和稳定命令同事务，不读取余额。

mod transaction;

use super::*;
use auction_application::{
    FundingCommitOutcome, FundingFuture, FundingReconciliationResult, FundingRecordOnlyCommand,
    FundingRecordOnlyStore, SettlementInternalEvidenceSnapshot,
};

impl FundingRecordOnlyStore for crate::DatabaseStore {
    fn replay_record_only(
        &self,
        command: FundingRecordOnlyCommand,
    ) -> FundingFuture<'_, Option<FundingReconciliationResult>> {
        Box::pin(async move {
            command.validate()?;
            let mut tx = self.postgres_pool().begin().await.map_err(storage)?;
            sqlx::query("SET TRANSACTION READ ONLY").execute(&mut *tx).await.map_err(storage)?;
            let result = transaction::replay(&mut tx, &command).await?;
            tx.commit().await.map_err(storage)?;
            Ok(result)
        })
    }
    fn initialize_record_only<'a>(
        &'a self,
        command: &'a FundingRecordOnlyCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, FundingCommitOutcome> {
        Box::pin(transaction::run(self.postgres_pool(), command, evidence))
    }
}
