//! SQLite 拍卖生命周期事务适配。
//!
//! 上下文与资格读取、生命周期计划写入、审计/realtime 记录分别由子模块拥有；父文件只装配
//! `AuctionLifecycleTransaction` 端口，SQLite 仍由父事务以 `BEGIN IMMEDIATE` 统一保护。

use super::*;

#[path = "lifecycle/context.rs"]
mod context;
#[path = "lifecycle/plan/mod.rs"]
mod plan;
#[path = "lifecycle/record.rs"]
mod record;

#[allow(clippy::manual_async_fn)]
impl AuctionLifecycleTransaction for SqliteCompetitionTransaction {
    fn load_lifecycle_context_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Option<AuctionLifecycleContext>, CompetitionTransactionError>> + Send
    {
        async move { context::load_lifecycle_context_sqlite(self, auction_id).await }
    }

    fn apply_lifecycle_plan(
        &mut self,
        command: &AuctionLifecycleCommand,
        plan: AuctionLifecyclePlan,
    ) -> impl Future<Output = Result<AuctionLifecycleWriteSummary, CompetitionTransactionError>> + Send
    {
        let command = command.clone();
        async move { plan::apply_lifecycle_plan_sqlite(self, &command, plan).await }
    }

    fn list_final_lot_ids_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Vec<LotId>, CompetitionTransactionError>> + Send {
        async move { context::list_final_lot_ids_for_update(self, auction_id).await }
    }

    fn load_final_lot_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<CompetitionSnapshot, CompetitionTransactionError>> + Send {
        async move { self.load_for_final_transition_inner(lot_id).await }
    }

    fn append_lifecycle_record(
        &mut self,
        command: &AuctionLifecycleCommand,
        context: &AuctionLifecycleContext,
        plan: &AuctionLifecyclePlan,
        result: &AuctionLifecycleResult,
        write_summary: AuctionLifecycleWriteSummary,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move {
            record::append_lifecycle_record_inner(
                self,
                command,
                context,
                plan,
                result,
                write_summary,
            )
            .await
        }
    }
}
