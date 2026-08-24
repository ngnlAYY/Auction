//! Admin/Worker 共享的拍卖生命周期用例。
//!
//! 父模块持有服务状态、命令幂等事务编排和端口实现；状态计划与后继轮次计算分别归档到
//! plan.rs 与 schedule.rs，不把领域状态机复制到应用层。

use std::sync::Arc;

use auction_domain::{
    AuctionId, BidId, CommandId, CompetitionPlan, CompetitionTrigger, ProxyMode, deterministic_uuid,
};
use chrono::{Datelike, Duration, NaiveDate};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    ApplicationError, ApplicationFuture, AuctionFinalProxyPlan, AuctionLifecycleCommand,
    AuctionLifecycleCommandPort, AuctionLifecycleContext, AuctionLifecycleEffect,
    AuctionLifecyclePlan, AuctionLifecyclePolicy, AuctionLifecycleResult,
    AuctionLifecycleTransaction, AuctionPreviewSuccessorPlan, CommandLedgerError,
    CompetitionTransaction, CompetitionTransactionError, RequestHash, StoredCommand,
    TransactionPolicy, TransactionRunner,
};

mod plan;
mod schedule;

use plan::lifecycle_plan;
use schedule::successor_plan;

/// Admin/Worker 共享的拍卖生命周期用例。
pub struct AuctionLifecycleService<R, C> {
    repository: Arc<R>,
    clock: Arc<C>,
    policy: AuctionLifecyclePolicy,
}

impl<R, C> AuctionLifecycleService<R, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction + AuctionLifecycleTransaction,
    C: crate::Clock,
{
    /// 建立只依赖事务 runner、时钟端口和启动期冻结策略的生命周期服务。
    pub fn new(repository: Arc<R>, clock: Arc<C>, policy: AuctionLifecyclePolicy) -> Self {
        Self { repository, clock, policy }
    }

    /// 在 Serializable 事务中查重、锁定聚合、应用领域迁移并追加账本记录。
    pub async fn execute(
        &self,
        command: AuctionLifecycleCommand,
    ) -> Result<AuctionLifecycleResult, ApplicationError> {
        if command.actor.actor_id.is_empty()
            || command.actor.actor_id != command.actor.actor_id.trim()
            || command.actor.actor_id.chars().any(char::is_control)
        {
            return Err(
                CompetitionTransactionError::Failed("生命周期命令发起者无效".to_owned()).into()
            );
        }
        let now = self.clock.now();
        let policy = self.policy;
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                let command = command.clone();
                Box::pin(async move {
                    let command_kind = command.action.command_kind();
                    if let Some(stored) = transaction.find_command(command.command_id).await? {
                        if !stored.matches_request(
                            command_kind,
                            command.auction_id.as_uuid(),
                            command.request_hash,
                        ) {
                            return Err(CommandLedgerError::Conflict.into());
                        }
                        if stored.result_code != "accepted" {
                            return Err(CompetitionTransactionError::Failed(
                                "生命周期命令账本结果分类不支持重放".to_owned(),
                            )
                            .into());
                        }
                        let replayed: AuctionLifecycleResult =
                            serde_json::from_slice(&stored.result_payload)?;
                        if replayed.command_id != command.command_id
                            || replayed.auction_id != command.auction_id
                        {
                            return Err(CompetitionTransactionError::Failed(
                                "生命周期命令账本结果聚合不一致".to_owned(),
                            )
                            .into());
                        }
                        return Ok(replayed);
                    }

                    let context = transaction
                        .load_lifecycle_context_for_update(command.auction_id)
                        .await?
                        .ok_or(CompetitionTransactionError::NotFound)?;
                    if command
                        .expected_state_version
                        .is_some_and(|version| version != context.auction.state_version)
                    {
                        return Err(CompetitionTransactionError::StaleState.into());
                    }
                    let plan = lifecycle_plan(transaction, &command, context, now, policy).await?;
                    let auction = &plan.auction;
                    let result = AuctionLifecycleResult {
                        command_id: command.command_id,
                        auction_id: AuctionId::from_uuid(auction.id.as_uuid()),
                        status: auction.status,
                        state_version: auction.state_version,
                        updated_at: auction.updated_at,
                    };
                    let stored = StoredCommand {
                        command_id: command.command_id,
                        command_kind: command.action.command_kind().to_owned(),
                        aggregate_id: command.auction_id.as_uuid(),
                        request_hash: command.request_hash,
                        result_code: "accepted".to_owned(),
                        result_payload: serde_json::to_vec(&result)?,
                    };
                    transaction.insert_command(stored).await?;
                    transaction.apply_lifecycle_plan(&command, plan).await?;
                    transaction.append_lifecycle_record(&command, &result).await?;
                    Ok(result)
                })
            })
            .await
    }
}

fn lifecycle_state(message: &str) -> ApplicationError {
    CompetitionTransactionError::Failed(message.to_owned()).into()
}

impl<R, C> AuctionLifecycleCommandPort for AuctionLifecycleService<R, C>
where
    R: TransactionRunner + 'static,
    R::Transaction: CompetitionTransaction + AuctionLifecycleTransaction,
    C: crate::Clock,
{
    fn allows_manual_early_open(&self) -> bool {
        self.policy.allows_manual_early_open()
    }

    fn execute(
        &self,
        command: AuctionLifecycleCommand,
    ) -> ApplicationFuture<'_, AuctionLifecycleResult> {
        Box::pin(async move { AuctionLifecycleService::execute(self, command).await })
    }
}
