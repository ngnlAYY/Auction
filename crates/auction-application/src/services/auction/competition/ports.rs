//! 竞价端口适配：把用例服务暴露为命令端口。

use auction_domain::CompetitionPlan;

use super::commands::{CreateProxyCommand, LowerProxyCommand, PlaceBidCommand};
use super::service::CompetitionService;
use crate::ports::auction::{
    CompetitionCommandOutcome, CompetitionCommandPort, ExecuteSlowProxyCommand,
    SlowProxyCommandPort,
};
use crate::ports::core::{CompetitionTransaction, TransactionRunner};

impl<R> CompetitionCommandPort for CompetitionService<R>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction,
{
    fn place_bid(
        &self,
        command: PlaceBidCommand,
    ) -> crate::ports::auction::ApplicationFuture<'_, CompetitionCommandOutcome> {
        Box::pin(async move { CompetitionService::place_bid(self, command).await })
    }

    fn create_proxy(
        &self,
        command: CreateProxyCommand,
    ) -> crate::ports::auction::ApplicationFuture<'_, CompetitionCommandOutcome> {
        Box::pin(async move { CompetitionService::create_proxy(self, command).await })
    }

    fn lower_proxy(
        &self,
        command: LowerProxyCommand,
    ) -> crate::ports::auction::ApplicationFuture<'_, CompetitionCommandOutcome> {
        Box::pin(async move { CompetitionService::lower_proxy(self, command).await })
    }
}

impl<R> SlowProxyCommandPort for CompetitionService<R>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction,
{
    fn execute_slow_proxy(
        &self,
        command: ExecuteSlowProxyCommand,
    ) -> crate::ports::auction::ApplicationFuture<'_, CompetitionPlan> {
        Box::pin(async move { CompetitionService::execute_slow_proxy(self, command).await })
    }
}
