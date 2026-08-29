//! 结算命令的应用层装配与事务编排。

use std::sync::Arc;

use auction_domain::{AuctionId, CommandId, SettlementPlan};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
    ApplicationError, ApplicationFuture, LegacySettlementVerification, RequestHash,
    SettlementSnapshot, SettlementTransaction, SettlementTransactionRunner, StoredCommand,
    TransactionPolicy,
};

mod legacy;
mod obligations;
mod validation;

use legacy::{StoredSettlementReplay, classify_stored_settlement, replay_legacy_settlement};
use obligations::append_obligations;
use validation::{plan_proxy_closures, validate_settlement_due, validate_settlement_plan};

/// 在已经取得一致只读快照后，严格核验旧版 complete 账本并返回脱敏摘要。
///
/// 调用方必须从同一数据库事务提供账本与结算快照。本函数没有“账本缺失则结算”的分支，
/// 因而不能被误用来补写 completion fence 或生成新的结算事实。
pub fn verify_legacy_settlement_replay(
    command: SettleAuctionCommand,
    stored: StoredCommand,
    snapshot: SettlementSnapshot,
) -> Result<LegacySettlementVerification, ApplicationError> {
    if snapshot.auction_id != command.auction_id || stored.command_kind != "settlement_complete" {
        return Err(legacy_verification_failed("只读核验只接受旧版已完成结算账本"));
    }
    let replay = classify_stored_settlement(&stored, command)?;
    let StoredSettlementReplay::Legacy(result) = replay else {
        return Err(legacy_verification_failed("只读核验拒绝新版结算账本"));
    };
    let plan = replay_legacy_settlement(command, &snapshot, result)?;
    let persisted = snapshot
        .persisted_settlement
        .as_ref()
        .ok_or_else(|| legacy_verification_failed("只读核验缺少权威结算事实"))?;
    let obligation_count = persisted
        .obligation_counts
        .iter()
        .try_fold(0_u64, |total, entry| total.checked_add(entry.count))
        .ok_or_else(|| legacy_verification_failed("只读核验义务数量溢出"))?;
    let line_count = u64::try_from(plan.lines.len())
        .map_err(|_| legacy_verification_failed("只读核验结算行数溢出"))?;
    let persisted_line_count = u64::try_from(persisted.lines.len())
        .map_err(|_| legacy_verification_failed("只读核验权威结算行数溢出"))?;
    let planned_obligation_count = u64::try_from(plan.obligations.len())
        .map_err(|_| legacy_verification_failed("只读核验计划义务数量溢出"))?;
    if line_count != persisted_line_count || obligation_count != planned_obligation_count {
        return Err(legacy_verification_failed("只读核验计划数量与权威事实不一致"));
    }

    Ok(LegacySettlementVerification {
        auction_id: command.auction_id.as_uuid(),
        workflow_id: command.workflow_id,
        claim_command_id: command.claim_command_id.as_uuid(),
        complete_command_id: command.command_id.as_uuid(),
        due_at: command.due_at,
        state_version: persisted.state_version,
        sold_count: persisted.sold_count,
        unsold_count: persisted.unsold_count,
        obligation_count,
        line_count,
    })
}

fn legacy_verification_failed(message: &str) -> ApplicationError {
    crate::SettlementTransactionError::Failed(message.to_owned()).into()
}

/// 请求一次拍卖结算。结算命令必须拥有稳定 ID，Worker 重试只能重放同一命令。
#[derive(Debug, Clone, Copy)]
pub struct SettleAuctionCommand {
    pub command_id: CommandId,
    pub workflow_id: Uuid,
    pub claim_command_id: CommandId,
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
                    if let Some(stored) = transaction.find_command(command.command_id).await? {
                        let replay = classify_stored_settlement(&stored, command)?;
                        let snapshot = transaction.load_for_update(command.auction_id).await?;
                        return match replay {
                            StoredSettlementReplay::Current => {
                                let replayed: SettlementPlan =
                                    serde_json::from_slice(&stored.result_payload)?;
                                validate_settlement_plan(command.command_id, &snapshot, &replayed)?;
                                Ok(replayed)
                            }
                            StoredSettlementReplay::Legacy(result) => {
                                replay_legacy_settlement(command, &snapshot, result)
                            }
                        };
                    }

                    let snapshot = transaction.load_for_update(command.auction_id).await?;
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
                    validate_settlement_plan(command.command_id, &snapshot, &plan)?;

                    transaction
                        .insert_command(
                            StoredCommand {
                                command_id: command.command_id,
                                command_kind: "settle_auction".to_owned(),
                                aggregate_id: command.auction_id.as_uuid(),
                                request_hash: command.request_hash,
                                result_code: "accepted".to_owned(),
                                result_payload: serde_json::to_vec(&plan)?,
                            },
                            command.now,
                        )
                        .await?;
                    transaction
                        .apply_settlement(
                            command.auction_id,
                            command.command_id,
                            plan.clone(),
                            proxy_closure_plans,
                            snapshot.recovery_policy,
                            command.now,
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
