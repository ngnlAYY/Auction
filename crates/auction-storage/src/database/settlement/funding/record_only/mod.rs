//! 仅账目初始化的SQL owner：真实身份、责任快照和稳定命令同事务，不读取余额。

mod transaction;

use auction_application::settlement::{
    FundingCommitOutcome, FundingFuture, FundingReconciliationResult, FundingRecordOnlyCommand,
    FundingRecordOnlyStore, SettlementInternalEvidenceSnapshot,
};

use crate::database::settlement::funding::storage;

impl FundingRecordOnlyStore for crate::DatabaseStore {
    fn replay_record_only(
        &self,
        command: FundingRecordOnlyCommand,
    ) -> FundingFuture<'_, Option<FundingReconciliationResult>> {
        Box::pin(self.tracked_postgres_pool().observe_transaction(
            auction_observability::DatabaseTransactionOwner::Funding,
            async move {
                command.validate()?;
                let mut tx = self
                    .tracked_postgres_pool()
                    .begin_with("BEGIN READ ONLY")
                    .await
                    .map_err(storage)?;
                let result = transaction::replay(&mut tx, &command).await?;
                tx.commit().await.map_err(storage)?;
                Ok(result)
            },
        ))
    }
    fn initialize_record_only<'a>(
        &'a self,
        command: &'a FundingRecordOnlyCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, FundingCommitOutcome> {
        Box::pin(transaction::run(self.tracked_postgres_pool(), command, evidence))
    }
}
