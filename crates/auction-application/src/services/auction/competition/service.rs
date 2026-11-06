//! 竞价用例服务：事务内锁定快照、校验命令并原子应用计划。

use std::sync::Arc;

use auction_domain::{
    AuctionStatus, CompetitionPlan, CompetitionTrigger, DomainError, ProxyMode, ProxyStatus,
};

use crate::errors::ApplicationError;
use crate::ports::auction::{
    CompetitionCommandOutcome, CompetitionWireTimestamp, ExecuteSlowProxyCommand,
    SLOW_PROXY_COMMAND_KIND,
};
use crate::ports::core::{
    CommandInsert, CompetitionTransaction, SlowProxyExecutionContext, StoredCommand,
    TransactionPolicy, TransactionRunner,
};

use super::commands::{
    CompetitionCommand, CreateProxyCommand, LowerProxyCommand, PlaceBidCommand, competition_bid_ids,
};
use super::legacy_ledger;
use super::replay::replay_stored_command;
use super::slow_replay;

/// 竞价用例服务，只编排端口和纯领域规划器；命令时刻只从锁后的事务端口读取。
pub struct CompetitionService<R> {
    repository: Arc<R>,
}

impl<R> CompetitionService<R>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction,
{
    /// 建立不持有具体数据库或网络实现的竞价用例。
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    /// 锁定竞价快照、计算计划并交给仓储在同一事务中原子应用。
    pub async fn place_bid(
        &self,
        command: PlaceBidCommand,
    ) -> Result<CompetitionCommandOutcome, ApplicationError> {
        self.execute(CompetitionCommand::PlaceBid(command)).await
    }

    /// 创建代理授权并在同一事务中应用它产生的初始竞价计划。
    pub async fn create_proxy(
        &self,
        command: CreateProxyCommand,
    ) -> Result<CompetitionCommandOutcome, ApplicationError> {
        self.execute(CompetitionCommand::CreateProxy(command)).await
    }

    /// 降低活动代理授权上限并追加转换事实。
    pub async fn lower_proxy(
        &self,
        command: LowerProxyCommand,
    ) -> Result<CompetitionCommandOutcome, ApplicationError> {
        self.execute(CompetitionCommand::LowerProxy(command)).await
    }

    /// 使用数据库时间推进一个持久化慢速代理 generation；过期候选返回稳定 stale 分类。
    pub async fn execute_slow_proxy(
        &self,
        command: ExecuteSlowProxyCommand,
    ) -> Result<CompetitionPlan, ApplicationError> {
        command.validate()?;
        let command_id = command.command_id;
        let lot_id = command.lot_id;
        let request_hash = command.request_hash;
        self.repository
            .run_in_auction_lot_queue(lot_id, TransactionPolicy::serializable(), |transaction| {
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command_id).await? {
                        if !stored.matches_request(
                            SLOW_PROXY_COMMAND_KIND,
                            lot_id.as_uuid(),
                            request_hash,
                        ) {
                            return Err(crate::ports::core::CommandLedgerError::Conflict.into());
                        }
                        if stored.result_code != "accepted" {
                            return Err(crate::ports::core::CompetitionTransactionError::Failed(
                                "慢速代理命令账本结果分类不支持重放".to_owned(),
                            )
                            .into());
                        }
                        let replayed: CompetitionPlan =
                            serde_json::from_slice(&stored.result_payload)?;
                        slow_replay::validate(&command, &replayed)?;
                        return Ok(replayed);
                    }

                    let snapshot = match transaction.load_for_update(lot_id).await {
                        Ok(snapshot) => snapshot,
                        Err(crate::ports::core::CompetitionTransactionError::NotFound) => {
                            return Err(
                                crate::ports::core::CompetitionTransactionError::StaleSlowProxy
                                    .into(),
                            );
                        }
                        Err(error) => return Err(error.into()),
                    };
                    let Some(active) = snapshot.active_proxy else {
                        return Err(
                            crate::ports::core::CompetitionTransactionError::StaleSlowProxy.into(),
                        );
                    };
                    let eligible =
                        transaction.participant_is_eligible_for_update(active.user_id).await?;
                    let now = transaction.current_time().await?;
                    if snapshot.auction_status != AuctionStatus::Live
                        || active.id != command.authorization_id
                        || active.mode != ProxyMode::Slow
                        || active.status != ProxyStatus::Active
                        || snapshot.state.slow_authorization_id != Some(command.authorization_id)
                        || snapshot.state.slow_generation != command.generation
                        || snapshot.state.slow_due_at != Some(command.due_at)
                        || now < command.due_at
                        || now >= snapshot.state.effective_end_at
                    {
                        return Err(
                            crate::ports::core::CompetitionTransactionError::StaleSlowProxy.into(),
                        );
                    }

                    let trigger = if eligible {
                        CompetitionTrigger::SlowDue {
                            command_id,
                            authorization_id: command.authorization_id,
                            generation: command.generation,
                            due_at: command.due_at,
                            bid_ids: competition_bid_ids(command_id, 1),
                            now,
                        }
                    } else {
                        CompetitionTrigger::FreezeProxy {
                            command_id,
                            authorization_id: command.authorization_id,
                            now,
                        }
                    };
                    let plan = CompetitionPlan::plan(
                        snapshot.rules,
                        snapshot.state,
                        Some(active),
                        trigger,
                    )
                    .map_err(map_slow_proxy_planning_error)?;
                    match transaction
                        .insert_command(StoredCommand {
                            command_id,
                            command_kind: SLOW_PROXY_COMMAND_KIND.to_owned(),
                            aggregate_id: lot_id.as_uuid(),
                            request_hash,
                            result_code: "accepted".to_owned(),
                            result_payload: serde_json::to_vec(&plan)?,
                        })
                        .await?
                    {
                        CommandInsert::Inserted => {}
                        // 并发同 ID 已提交：返回账本里已经应用的同一计划，不重复推进。
                        CommandInsert::Duplicate(stored) => {
                            if stored.result_code != "accepted" {
                                return Err(
                                    crate::ports::core::CompetitionTransactionError::Failed(
                                        "慢速代理命令账本结果分类不支持重放".to_owned(),
                                    )
                                    .into(),
                                );
                            }
                            let replayed: CompetitionPlan =
                                serde_json::from_slice(&stored.result_payload)?;
                            slow_replay::validate(&command, &replayed)?;
                            return Ok(replayed);
                        }
                    }
                    transaction
                        .apply_slow_proxy_plan(
                            &plan,
                            SlowProxyExecutionContext {
                                authorization_id: command.authorization_id,
                                generation: command.generation,
                                due_at: command.due_at,
                                executed_at: now,
                            },
                        )
                        .await?;
                    Ok(plan)
                })
            })
            .await
    }

    async fn execute(
        &self,
        command: CompetitionCommand,
    ) -> Result<CompetitionCommandOutcome, ApplicationError> {
        let command_id = command.command_id();
        let lot_id = command.lot_id();
        let request_hash = command.request_hash();
        let command_kind = command.kind();
        self.repository
            .run_in_auction_lot_queue(lot_id, TransactionPolicy::serializable(), |transaction| {
                let command = command.clone();
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command_id).await? {
                        return replay_stored_command(transaction, &command, stored).await;
                    }

                    let mut snapshot = transaction.load_for_update(lot_id).await?;
                    let quota_limit = snapshot.rules.proxy_max_outbid_uses;
                    if let CompetitionCommand::CreateProxy(create) = &command {
                        CompetitionPlan::validate_proxy_creation_policy(
                            snapshot.rules,
                            snapshot.auction_status,
                            create.mode.unwrap_or(snapshot.rules.proxy_default_mode),
                        )?;
                    }
                    if let CompetitionCommand::LowerProxy(lower) = &command {
                        CompetitionPlan::validate_lower_proxy_target(
                            snapshot.active_proxy,
                            lower.bidder_id,
                            lower.authorization_id,
                        )?;
                    }
                    let proxy_usage_context = match &command {
                        CompetitionCommand::CreateProxy(create) => {
                            let context =
                                transaction.proxy_usage_context(lot_id, create.bidder_id).await?;
                            CompetitionPlan::validate_proxy_creation_history(
                                snapshot.rules,
                                context.usage_number,
                                context.previous_maximum,
                                context.previous_status,
                                create.maximum_amount,
                            )?;
                            Some(context)
                        }
                        CompetitionCommand::PlaceBid(_) | CompetitionCommand::LowerProxy(_) => None,
                    };
                    let previous_proxy_transitions = match &command {
                        CompetitionCommand::LowerProxy(lower) => {
                            transaction.proxy_transition_history(lower.authorization_id).await?
                        }
                        CompetitionCommand::PlaceBid(_) | CompetitionCommand::CreateProxy(_) => {
                            Vec::new()
                        }
                    };
                    let bidder_id = command.bidder_id();
                    // 资格参与者最多两个；栈内排序去重仍保持原 UUID 加锁顺序。
                    let mut participants = [
                        bidder_id,
                        snapshot.active_proxy.map_or(bidder_id, |active| active.user_id),
                    ];
                    participants.sort_unstable();
                    let participant_count = 1 + usize::from(participants[0] != participants[1]);

                    let mut bidder_eligible = false;
                    let mut active_proxy_eligible = true;
                    for &participant_id in &participants[..participant_count] {
                        let eligible =
                            transaction.participant_is_eligible_for_update(participant_id).await?;
                        if participant_id == bidder_id {
                            bidder_eligible = eligible;
                        }
                        if snapshot
                            .active_proxy
                            .is_some_and(|active| active.user_id == participant_id)
                        {
                            active_proxy_eligible = eligible;
                        }
                    }
                    if !bidder_eligible {
                        return Err(
                            crate::ports::core::CompetitionTransactionError::BidderIneligible
                                .into(),
                        );
                    }

                    // 用户资格锁同样可能等待；每次序列化重试都在全部锁齐备后重新计时。
                    let now = transaction.current_time().await?;
                    if now >= snapshot.state.effective_end_at {
                        return Err(
                            crate::ports::core::CompetitionTransactionError::InvalidState.into()
                        );
                    }
                    let audit = command.audit_context(now);
                    let prerequisite_plan = if let Some(active) = snapshot.active_proxy
                        && active.user_id != bidder_id
                        && !active_proxy_eligible
                    {
                        let freeze = CompetitionPlan::plan(
                            snapshot.rules,
                            snapshot.state,
                            Some(active),
                            CompetitionTrigger::FreezeProxy {
                                command_id,
                                authorization_id: active.id,
                                now,
                            },
                        )?;
                        snapshot.state = freeze.final_lot;
                        snapshot.active_proxy = None;
                        Some(freeze)
                    } else {
                        None
                    };
                    let trigger = command.trigger(&snapshot, now, proxy_usage_context)?;
                    let plan = CompetitionPlan::plan(
                        snapshot.rules,
                        snapshot.state,
                        snapshot.active_proxy,
                        trigger,
                    )?;
                    let legacy_ledger::EncodedCommandResult { payload, proxy } =
                        legacy_ledger::encode(
                            &command,
                            &plan,
                            &previous_proxy_transitions,
                            quota_limit,
                            now,
                        )?;
                    match transaction
                        .insert_command(StoredCommand {
                            command_id,
                            command_kind: command_kind.to_owned(),
                            aggregate_id: lot_id.as_uuid(),
                            request_hash,
                            result_code: legacy_ledger::LEGACY_RESULT_CODE.to_owned(),
                            result_payload: payload,
                        })
                        .await?
                    {
                        CommandInsert::Inserted => {}
                        // 并发同 ID 已提交：按账本结果重放，不再应用本事务计划。
                        CommandInsert::Duplicate(stored) => {
                            return replay_stored_command(transaction, &command, stored).await;
                        }
                    }
                    if let Some(prerequisite) = prerequisite_plan {
                        transaction.apply_plan(&prerequisite, None, now).await?;
                    }
                    transaction.apply_plan(&plan, Some(audit), now).await?;
                    let effective_end_at =
                        CompetitionWireTimestamp::from_utc(plan.final_lot.effective_end_at);
                    Ok(CompetitionCommandOutcome { plan, proxy, effective_end_at })
                })
            })
            .await
    }
}

fn map_slow_proxy_planning_error(error: DomainError) -> ApplicationError {
    match error {
        DomainError::AuctionClosed
        | DomainError::InvalidBidState(_)
        | DomainError::InvalidProxyTransition { .. } => {
            crate::ports::core::CompetitionTransactionError::StaleSlowProxy.into()
        }
        error => error.into(),
    }
}
