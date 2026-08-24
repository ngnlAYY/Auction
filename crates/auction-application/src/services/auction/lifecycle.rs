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
    AuctionLifecyclePlan, AuctionLifecycleResult, AuctionLifecycleTransaction,
    AuctionPreviewSuccessorPlan, CommandLedgerError, CompetitionTransaction,
    CompetitionTransactionError, RequestHash, StoredCommand, TransactionPolicy, TransactionRunner,
};

/// Admin/Worker 共享的拍卖生命周期用例。
pub struct AuctionLifecycleService<R, C> {
    repository: Arc<R>,
    clock: Arc<C>,
}

impl<R, C> AuctionLifecycleService<R, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction + AuctionLifecycleTransaction,
    C: crate::Clock,
{
    /// 建立只依赖事务 runner 和时钟端口的生命周期服务。
    pub fn new(repository: Arc<R>, clock: Arc<C>) -> Self {
        Self { repository, clock }
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
                    let plan = lifecycle_plan(transaction, &command, context, now).await?;
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

async fn lifecycle_plan<T>(
    transaction: &mut T,
    command: &AuctionLifecycleCommand,
    context: AuctionLifecycleContext,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<AuctionLifecyclePlan, ApplicationError>
where
    T: CompetitionTransaction + AuctionLifecycleTransaction,
{
    let mut auction = context.auction.clone();
    let admin_action = command.actor.actor_type == crate::AuctionLifecycleActorType::Admin;
    let effect = match command.action {
        crate::AuctionLifecycleAction::Preview => {
            if !admin_action && now < context.preview_scheduled_at {
                return Err(CompetitionTransactionError::InvalidState.into());
            }
            auction.preview(now)?;
            AuctionLifecycleEffect::Preview {
                manual: admin_action,
                successor: successor_plan(&context)?,
            }
        }
        crate::AuctionLifecycleAction::Open => {
            let manual_override = admin_action && now < context.scheduled_open_at;
            if !manual_override
                && (now < context.preview_scheduled_at || now < context.scheduled_open_at)
            {
                return Err(CompetitionTransactionError::InvalidState.into());
            }
            auction.open(context.policy_scheduled_end_at, now)?;
            AuctionLifecycleEffect::Open { manual_override }
        }
        crate::AuctionLifecycleAction::Cancel => {
            let reason = command
                .cancel_reason
                .clone()
                .unwrap_or_else(|| "Worker lifecycle cancellation".to_owned());
            auction.cancel_before_open_with_metadata(
                now,
                reason,
                command.actor.actor_id.clone(),
            )?;
            AuctionLifecycleEffect::Cancel
        }
        crate::AuctionLifecycleAction::FinalPhase => {
            let (proxy_plans, earliest_end_at) =
                final_proxy_plans(transaction, command, now).await?;
            if !admin_action {
                let due_at = earliest_end_at
                    .checked_sub_signed(context.policy_final_phase_lead)
                    .ok_or_else(|| lifecycle_state("最终阶段到期时间超出范围"))?;
                if now < due_at {
                    return Err(CompetitionTransactionError::InvalidState.into());
                }
            }
            auction.enter_final(now)?;
            AuctionLifecycleEffect::FinalPhase { proxy_plans }
        }
    };
    Ok(AuctionLifecyclePlan { auction, effect })
}

async fn final_proxy_plans<T>(
    transaction: &mut T,
    parent: &AuctionLifecycleCommand,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(Vec<AuctionFinalProxyPlan>, chrono::DateTime<chrono::Utc>), ApplicationError>
where
    T: CompetitionTransaction + AuctionLifecycleTransaction,
{
    let lot_ids = transaction.list_final_lot_ids_for_update(parent.auction_id).await?;
    if lot_ids.is_empty() {
        return Err(CompetitionTransactionError::InvalidState.into());
    }
    let mut plans = Vec::new();
    let mut earliest_end_at: Option<chrono::DateTime<chrono::Utc>> = None;
    for lot_id in lot_ids {
        let snapshot = transaction.load_for_update(lot_id).await?;
        earliest_end_at =
            Some(earliest_end_at.map_or(snapshot.state.effective_end_at, |current| {
                current.min(snapshot.state.effective_end_at)
            }));
        let Some(active) = snapshot.active_proxy else {
            continue;
        };
        let eligible = transaction.proxy_user_is_eligible_for_update(active.user_id).await?;
        if eligible && active.mode == ProxyMode::Fast {
            continue;
        }

        let child_command_id = CommandId::from_uuid(deterministic_uuid(
            parent.command_id,
            &format!("auction-final-proxy:{lot_id}"),
            1,
        ));
        let trigger = if eligible {
            let bid_ids = if snapshot.state.slow_authorization_id == Some(active.id) {
                vec![BidId::from_uuid(deterministic_uuid(child_command_id, "auction-bid-fact", 1))]
            } else {
                Vec::new()
            };
            CompetitionTrigger::FinalProxy {
                command_id: child_command_id,
                authorization_id: active.id,
                bid_ids,
                now,
            }
        } else {
            CompetitionTrigger::FreezeProxy {
                command_id: child_command_id,
                authorization_id: active.id,
                now,
            }
        };
        let request_hash =
            RequestHash::from_request("final_proxy_transition", lot_id.as_uuid(), &trigger)?;
        let competition =
            CompetitionPlan::plan(snapshot.rules, snapshot.state, Some(active), trigger)?;
        plans.push(AuctionFinalProxyPlan {
            command: StoredCommand {
                command_id: child_command_id,
                command_kind: "final_proxy_transition".to_owned(),
                aggregate_id: lot_id.as_uuid(),
                request_hash,
                result_code: "accepted".to_owned(),
                result_payload: serde_json::to_vec(&competition)?,
            },
            competition,
        });
    }
    let earliest_end_at =
        earliest_end_at.ok_or_else(|| lifecycle_state("最终阶段缺少活动标的结束时间"))?;
    Ok((plans, earliest_end_at))
}

fn successor_plan(
    context: &AuctionLifecycleContext,
) -> Result<AuctionPreviewSuccessorPlan, ApplicationError> {
    let current_week = context.scheduled_round_local_week.unwrap_or_else(|| {
        context.preview_scheduled_at.date_naive()
            - Duration::days(i64::from(
                context.preview_scheduled_at.weekday().num_days_from_monday(),
            ))
    });
    let local_week = checked_add_days(current_week, 7, "下一期周日期超出范围")?;
    let preview_due_at = checked_add_week(context.preview_scheduled_at, "下一期预览时间超出范围")?;
    let open_due_at = checked_add_week(context.scheduled_open_at, "下一期开拍时间超出范围")?;
    let scheduled_end_at =
        checked_add_week(context.policy_scheduled_end_at, "下一期结束时间超出范围")?;
    let sequence =
        context.sequence.checked_add(1).ok_or_else(|| lifecycle_state("下一期拍卖序号溢出"))?;
    let local_week_text = local_week.format("%Y-%m-%d").to_string();
    let round_id = schedule_id("round", &context.world_id, &local_week_text);
    let auction_id =
        AuctionId::from_uuid(schedule_id("auction", &context.world_id, &local_week_text));
    let create_command_id =
        CommandId::from_uuid(schedule_id("create-command", &context.world_id, &local_week_text));
    let title = format!("{} #{}", context.world_id, sequence);

    #[derive(Serialize)]
    struct ScheduledCreateRequest<'a> {
        world_id: &'a str,
        sequence: i64,
        local_week: &'a str,
        preview_due_at: chrono::DateTime<chrono::Utc>,
        open_due_at: chrono::DateTime<chrono::Utc>,
        scheduled_end_at: chrono::DateTime<chrono::Utc>,
    }
    let create_request_hash = RequestHash::from_request(
        "scheduled_auction_create",
        auction_id.as_uuid(),
        &ScheduledCreateRequest {
            world_id: &context.world_id,
            sequence,
            local_week: &local_week_text,
            preview_due_at,
            open_due_at,
            scheduled_end_at,
        },
    )?;

    Ok(AuctionPreviewSuccessorPlan {
        round_id,
        auction_id,
        create_command_id,
        create_request_hash,
        local_week,
        sequence,
        title,
        preview_due_at,
        open_due_at,
        scheduled_end_at,
    })
}

fn checked_add_week(
    value: chrono::DateTime<chrono::Utc>,
    message: &str,
) -> Result<chrono::DateTime<chrono::Utc>, ApplicationError> {
    value.checked_add_signed(Duration::days(7)).ok_or_else(|| lifecycle_state(message))
}

fn checked_add_days(
    value: NaiveDate,
    days: i64,
    message: &str,
) -> Result<NaiveDate, ApplicationError> {
    value.checked_add_signed(Duration::days(days)).ok_or_else(|| lifecycle_state(message))
}

/// 保持与旧版周计划相同的长度前缀 SHA-256 标识算法，避免迁移后同一 world/week 产生双轮次。
fn schedule_id(kind: &str, world_id: &str, local_week: &str) -> Uuid {
    let mut digest = Sha256::new();
    for value in ["auction-schedule", kind, world_id, local_week] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
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
    fn execute(
        &self,
        command: AuctionLifecycleCommand,
    ) -> ApplicationFuture<'_, AuctionLifecycleResult> {
        Box::pin(async move { AuctionLifecycleService::execute(self, command).await })
    }
}
