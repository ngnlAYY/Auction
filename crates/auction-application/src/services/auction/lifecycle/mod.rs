//! Admin/Worker 共享的拍卖生命周期用例。
//! 父模块持有服务状态、命令幂等事务编排和端口实现；状态计划与后继轮次计算分别归档到
//! plan.rs 与 schedule.rs，不把领域状态机复制到应用层。

use std::sync::Arc;

use auction_domain::{
    AuctionId, AuctionScheduleCalendar, BidId, CommandId, CompetitionPlan, CompetitionTrigger,
    ProxyMode, deterministic_uuid,
};
use chrono::{Datelike, Duration, NaiveDate};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::ports::auction::{
    ApplicationFuture, AuctionFinalProxyPlan, AuctionLifecycleCommand, AuctionLifecycleCommandPort,
    AuctionLifecycleContext, AuctionLifecycleEffect, AuctionLifecyclePlan, AuctionLifecyclePolicy,
    AuctionLifecycleResult, AuctionLifecycleSnapshot, AuctionLifecycleTransaction,
    AuctionPolicyTemplate, AuctionPreviewSuccessorPlan,
};
use crate::ports::core::{
    CommandLedgerError, CompetitionTransaction, CompetitionTransactionError, RequestHash,
    StoredCommand, TransactionPolicy, TransactionRunner,
};

mod plan;
mod schedule;

use plan::lifecycle_plan;
use schedule::successor_plan;

/// Admin/Worker 共享的拍卖生命周期用例。
pub struct AuctionLifecycleService<R> {
    repository: Arc<R>,
    policy: AuctionLifecyclePolicy,
    successor_policy: AuctionPolicyTemplate,
    /// 启动期有效的周排期与早鸟日历；缺失时后继保持冻结来源的七天规则。
    schedule_calendar: Option<AuctionScheduleCalendar>,
}

impl<R> AuctionLifecycleService<R>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction + AuctionLifecycleTransaction,
{
    /// 建立只依赖事务 runner 和启动期冻结策略的服务；命令时刻由锁后的事务端口提供。
    pub fn new(
        repository: Arc<R>,
        policy: AuctionLifecyclePolicy,
        successor_policy: AuctionPolicyTemplate,
    ) -> Self {
        Self { repository, policy, successor_policy, schedule_calendar: None }
    }

    /// 注入启动期有效的排期日历；只影响此后新建的后继，不改写已冻结轮次。
    pub fn with_schedule_calendar(mut self, calendar: Option<AuctionScheduleCalendar>) -> Self {
        self.schedule_calendar = calendar;
        self
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
        let policy = self.policy;
        // 生命周期与同场次竞价共用一把按场次的门：迁移写 `auctions`/`lots`，与 Final 出价的
        // 重叠会让 SSI 以 40001 互相中止。排队在 `BEGIN` 之前完成，隔离级别与锁序不变。
        let transaction_policy = TransactionPolicy::serializable();
        self.repository
            .run_in_auction_queue(command.auction_id, transaction_policy, |transaction| {
                let command = command.clone();
                let successor_policy = self.successor_policy.clone();
                let schedule_calendar = self.schedule_calendar.clone();
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
                        let mut replayed: AuctionLifecycleResult =
                            serde_json::from_slice(&stored.result_payload)?;
                        if replayed.command_id != command.command_id
                            || replayed.auction_id != command.auction_id
                        {
                            return Err(CompetitionTransactionError::Failed(
                                "生命周期命令账本结果聚合不一致".to_owned(),
                            )
                            .into());
                        }
                        if let Some(snapshot) = replayed.auction.as_ref()
                            && (snapshot.id != replayed.auction_id
                                || snapshot.status != replayed.status
                                || snapshot.state_version != replayed.state_version
                                || snapshot.updated_at != replayed.updated_at)
                        {
                            return Err(CompetitionTransactionError::Failed(
                                "生命周期命令账本完整快照不一致".to_owned(),
                            )
                            .into());
                        }
                        if replayed.auction.is_none()
                            && let Some(context) = transaction
                                .load_lifecycle_context_for_update(command.auction_id)
                                .await?
                            && context.auction.status == replayed.status
                            && context.auction.state_version == replayed.state_version
                            && context.auction.updated_at == replayed.updated_at
                        {
                            // 只在数据库仍停留于旧最小结果对应版本时补出精确快照；状态已继续推进时
                            // 宁可保留 `None` 让 HTTP 失败关闭，也不把当前状态伪装成历史幂等结果。
                            replayed.auction = Some(snapshot_from_context(&context));
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
                    let plan = lifecycle_plan(
                        transaction,
                        &command,
                        &context,
                        policy,
                        &successor_policy,
                        schedule_calendar.as_ref(),
                    )
                    .await?;
                    let result = lifecycle_result(&command, &context, &plan);
                    let stored = StoredCommand {
                        command_id: command.command_id,
                        command_kind: command.action.command_kind().to_owned(),
                        aggregate_id: command.auction_id.as_uuid(),
                        request_hash: command.request_hash,
                        result_code: "accepted".to_owned(),
                        result_payload: serde_json::to_vec(&result)?,
                    };
                    transaction.insert_command(stored).await?;
                    let write_summary = transaction.apply_lifecycle_plan(&command, &plan).await?;
                    transaction
                        .append_lifecycle_record(&command, &context, &plan, &result, write_summary)
                        .await?;
                    Ok(result)
                })
            })
            .await
    }
}

fn lifecycle_result(
    command: &AuctionLifecycleCommand,
    context: &AuctionLifecycleContext,
    plan: &AuctionLifecyclePlan,
) -> AuctionLifecycleResult {
    let auction = &plan.auction;
    let mut snapshot = snapshot_from_context(context);
    snapshot.id = AuctionId::from_uuid(auction.id.as_uuid());
    snapshot.status = auction.status;
    snapshot.state_version = auction.state_version;
    snapshot.updated_at = auction.updated_at;
    snapshot.cancelled_at = auction.cancelled_at;
    snapshot.cancelled_reason.clone_from(&auction.cancelled_reason);
    snapshot.cancelled_by.clone_from(&auction.cancelled_by);

    match &plan.effect {
        AuctionLifecycleEffect::Preview { manual, .. } => {
            snapshot.preview_opened_at = snapshot.preview_opened_at.or(Some(auction.updated_at));
            snapshot.preview_manual_open |= *manual;
            // 预览开放只推进生命周期版本；内容修订号已经由录入、lot 编辑或重列命令推进。
        }
        AuctionLifecycleEffect::Open { manual_override, .. } => {
            snapshot.preview_opened_at = snapshot.preview_opened_at.or(Some(auction.updated_at));
            if *manual_override {
                if snapshot.preview_scheduled_at > auction.updated_at {
                    snapshot.preview_scheduled_at = auction.updated_at;
                }
                snapshot.scheduled_open_at = auction.updated_at;
            }
            snapshot.opened_at = Some(auction.updated_at);
        }
        AuctionLifecycleEffect::Cancel { .. } => {}
        AuctionLifecycleEffect::FinalPhase { .. } => {
            snapshot.final_phase_at = Some(auction.updated_at);
        }
        AuctionLifecycleEffect::EarlySettlement { .. } => {
            snapshot.final_phase_at = snapshot.final_phase_at.or(Some(auction.updated_at));
        }
    }

    AuctionLifecycleResult {
        command_id: command.command_id,
        auction_id: AuctionId::from_uuid(auction.id.as_uuid()),
        status: auction.status,
        state_version: auction.state_version,
        updated_at: auction.updated_at,
        auction: Some(snapshot),
    }
}

fn snapshot_from_context(context: &AuctionLifecycleContext) -> AuctionLifecycleSnapshot {
    AuctionLifecycleSnapshot {
        id: AuctionId::from_uuid(context.auction.id.as_uuid()),
        world_id: context.world_id.clone(),
        sequence: context.sequence,
        status: context.auction.status,
        preview_scheduled_at: context.preview_scheduled_at,
        scheduled_open_at: context.scheduled_open_at,
        preview_opened_at: context.preview_opened_at,
        preview_manual_open: context.preview_manual_open,
        preview_revision: context.preview_revision,
        opened_at: context.opened_at,
        final_phase_at: context.final_phase_at,
        settled_at: context.settled_at,
        cancelled_at: context.auction.cancelled_at,
        cancelled_reason: context.auction.cancelled_reason.clone(),
        cancelled_by: context.auction.cancelled_by.clone(),
        state_version: context.auction.state_version,
        created_at: context.auction.created_at,
        updated_at: context.auction.updated_at,
    }
}

fn lifecycle_state(message: &str) -> ApplicationError {
    CompetitionTransactionError::Failed(message.to_owned()).into()
}

impl<R> AuctionLifecycleCommandPort for AuctionLifecycleService<R>
where
    R: TransactionRunner + 'static,
    R::Transaction: CompetitionTransaction + AuctionLifecycleTransaction,
{
    fn allows_manual_early_open(&self) -> bool {
        self.policy.allows_manual_early_open()
    }

    fn allows_manual_early_settlement(&self) -> bool {
        self.policy.allows_manual_early_settlement()
    }

    fn execute(
        &self,
        command: AuctionLifecycleCommand,
    ) -> ApplicationFuture<'_, AuctionLifecycleResult> {
        Box::pin(async move { AuctionLifecycleService::execute(self, command).await })
    }
}
