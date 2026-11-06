//! 结算命令的应用层装配与事务编排。

use std::sync::Arc;

use auction_domain::{AuctionId, CommandId, SettlementPlan};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::ports::auction::ApplicationFuture;
use crate::ports::core::{RequestHash, StoredCommand, TransactionPolicy};
use crate::ports::settlement::{SettlementTransaction, SettlementTransactionRunner};

mod legacy;
mod obligations;
mod validation;

use legacy::{StoredSettlementReplay, classify_stored_settlement, replay_legacy_settlement};
use obligations::build_obligations;
use validation::{
    plan_proxy_closures, validate_current_settlement_identity, validate_replayed_settlement_plan,
    validate_settlement_due, validate_settlement_plan,
};

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

/// 从拍卖与截止世代派生结算工作流身份，与旧 Worker 字节编码一致。
pub fn settlement_workflow_id(auction_id: Uuid, due_at: DateTime<Utc>) -> Option<Uuid> {
    legacy_worker_id("auction-settlement", "workflow", auction_id, due_at)
}

/// 从拍卖与截止世代派生领取命令身份。
pub fn settlement_claim_command_id(auction_id: Uuid, due_at: DateTime<Utc>) -> Option<CommandId> {
    legacy_worker_id("auction-worker", "settlement_claim", auction_id, due_at)
        .map(CommandId::from_uuid)
}

/// 从拍卖与截止世代派生完成命令身份。
pub fn settlement_complete_command_id(
    auction_id: Uuid,
    due_at: DateTime<Utc>,
) -> Option<CommandId> {
    legacy_worker_id("auction-worker", "settlement_complete", auction_id, due_at)
        .map(CommandId::from_uuid)
}

/// 按旧 Worker 的 `{workflow_id,due_at}` 字段顺序生成完成命令摘要。
pub fn settlement_complete_request_hash(
    auction_id: Uuid,
    workflow_id: Uuid,
    due_at: DateTime<Utc>,
) -> Result<RequestHash, serde_json::Error> {
    #[derive(Serialize)]
    struct SettlementWorkflowRequest {
        workflow_id: Uuid,
        due_at: DateTime<Utc>,
    }

    RequestHash::from_request(
        "settle_auction",
        auction_id,
        &SettlementWorkflowRequest { workflow_id, due_at },
    )
}

/// 逐字复现旧 Worker 的长度前缀 SHA-256 身份，due 世代必须参与 workflow 和两个命令 ID。
fn legacy_worker_id(
    namespace: &str,
    kind: &str,
    auction_id: Uuid,
    due_at: DateTime<Utc>,
) -> Option<Uuid> {
    let mut hasher = Sha256::new();
    for value in [namespace.as_bytes(), kind.as_bytes(), auction_id.as_bytes()] {
        hasher.update(u64::try_from(value.len()).ok()?.to_be_bytes());
        hasher.update(value);
    }
    let due_nanos = due_at.timestamp_nanos_opt()?;
    hasher.update((due_nanos as u64).to_be_bytes());
    let mut bytes: [u8; 16] = hasher.finalize()[..16].try_into().ok()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Some(Uuid::from_bytes(bytes))
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
                                validate_current_settlement_identity(command, &snapshot)?;
                                let replayed: SettlementPlan =
                                    serde_json::from_slice(&stored.result_payload)?;
                                validate_replayed_settlement_plan(
                                    command.command_id,
                                    &snapshot,
                                    &replayed,
                                )?;
                                Ok(replayed)
                            }
                            StoredSettlementReplay::Legacy(result) => {
                                replay_legacy_settlement(command, &snapshot, result)
                            }
                        };
                    }

                    let snapshot = transaction.load_for_update(command.auction_id).await?;
                    // 首次写入先核对锁内世代、稳定身份和到期，不能让过期命令计算或提交新计划。
                    validate_current_settlement_identity(command, &snapshot)?;
                    validate_settlement_due(command, &snapshot)?;
                    let mut plan = SettlementPlan::calculate(
                        &snapshot.policy,
                        snapshot.scheduled_open_at,
                        snapshot.scheduled_end_at,
                        snapshot.opening_pool,
                        &snapshot.facts,
                    )?;
                    let proxy_closure_plans = plan_proxy_closures(command, &snapshot)?;
                    plan.obligations = build_obligations(command.command_id, &snapshot, &plan)?;
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
                            &plan,
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
