//! 结算命令的应用层装配与事务编排。

use std::sync::Arc;

use auction_domain::{AuctionId, CommandId, SettlementPlan};
use chrono::{DateTime, Utc};

use crate::{
    ApplicationError, ApplicationFuture, RequestHash, SettlementTransaction,
    SettlementTransactionRunner, StoredCommand, TransactionPolicy,
};

mod obligations;
mod validation;

use obligations::append_obligations;
use validation::{plan_proxy_closures, validate_settlement_due};

/// 请求一次拍卖结算。结算命令必须拥有稳定 ID，Worker 重试只能重放同一命令。
#[derive(Debug, Clone, Copy)]
pub struct SettleAuctionCommand {
    pub command_id: CommandId,
    pub auction_id: AuctionId,
    /// 本次 workflow 冻结的最晚 lot 有效结束时间。
    pub due_at: DateTime<Utc>,
    /// Worker 本次执行时间，只用于到期校验和代理终态事实。
    pub now: DateTime<Utc>,
    pub request_hash: RequestHash,
}

/// 结算命令的窄应用端口，供 Admin 和 Worker 共享同一幂等用例。
pub trait SettlementCommandPort: Send + Sync {
    fn settle(&self, command: SettleAuctionCommand) -> ApplicationFuture<'_, SettlementPlan>;
}

/// 结算用例服务，只编排锁内事实读取、领域计算和持久化事务。
pub struct SettlementService<R> {
    repository: Arc<R>,
}

impl<R> SettlementService<R>
where
    R: SettlementTransactionRunner,
    R::Transaction: SettlementTransaction,
{
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    /// 在 Serializable 事务中生成并保存结算事实、义务和命令账本。
    pub async fn settle(
        &self,
        command: SettleAuctionCommand,
    ) -> Result<SettlementPlan, ApplicationError> {
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                Box::pin(async move {
                    let snapshot = transaction.load_for_update(command.auction_id).await?;
                    if let Some(stored) = transaction.find_command(command.command_id).await? {
                        if !stored.matches_request(
                            "settle_auction",
                            command.auction_id.as_uuid(),
                            command.request_hash,
                        ) {
                            return Err(crate::CommandLedgerError::Conflict.into());
                        }
                        if stored.result_code != "accepted" {
                            return Err(crate::SettlementTransactionError::Failed(
                                "命令账本结果分类不支持结算重放".to_owned(),
                            )
                            .into());
                        }
                        let replayed: SettlementPlan =
                            serde_json::from_slice(&stored.result_payload)?;
                        replayed.validate(&snapshot.policy)?;
                        return Ok(replayed);
                    }

                    let mut plan = SettlementPlan::calculate(
                        &snapshot.policy,
                        snapshot.scheduled_open_at,
                        snapshot.scheduled_end_at,
                        snapshot.opening_pool,
                        &snapshot.facts,
                    )?;
                    validate_settlement_due(command, &snapshot)?;
                    let proxy_closure_plans = plan_proxy_closures(command, &snapshot)?;
                    append_obligations(command.command_id, &snapshot, &mut plan)?;
                    plan.validate(&snapshot.policy)?;

                    transaction
                        .insert_command(StoredCommand {
                            command_id: command.command_id,
                            command_kind: "settle_auction".to_owned(),
                            aggregate_id: command.auction_id.as_uuid(),
                            request_hash: command.request_hash,
                            result_code: "accepted".to_owned(),
                            result_payload: serde_json::to_vec(&plan)?,
                        })
                        .await?;
                    transaction
                        .apply_settlement(
                            command.auction_id,
                            command.command_id,
                            plan.clone(),
                            proxy_closure_plans,
                            snapshot.recovery_policy,
                        )
                        .await?;
                    Ok(plan)
                })
            })
            .await
    }
}

impl<R> SettlementCommandPort for SettlementService<R>
where
    R: SettlementTransactionRunner + 'static,
    R::Transaction: SettlementTransaction,
{
    fn settle(&self, command: SettleAuctionCommand) -> ApplicationFuture<'_, SettlementPlan> {
        Box::pin(async move { SettlementService::settle(self, command).await })
    }
}
