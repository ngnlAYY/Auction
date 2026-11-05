use std::sync::Arc;

use auction_domain::DomainError;

use crate::{
    ADMIN_AUCTION_SCHEDULE_COMMAND_KIND, AdminAuctionSchedulePort, AdminAuctionScheduleTransaction,
    AdminAuctionSummary, AdminUpdateAuctionScheduleCommand, ApplicationError, ApplicationFuture,
    CommandLedgerError, CompetitionTransaction, CompetitionTransactionError, StoredCommand,
    TransactionPolicy, TransactionRunner,
};

/// Admin 正式开拍前修改预览、开拍与计划结束时间的用例。
pub struct AdminAuctionScheduleService<R> {
    repository: Arc<R>,
}

impl<R> AdminAuctionScheduleService<R>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction + AdminAuctionScheduleTransaction,
{
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    /// 在同一事务中锁定计划、校验 Draft/Preview 状态并同步调度记录。
    pub async fn update_schedule(
        &self,
        command: AdminUpdateAuctionScheduleCommand,
    ) -> Result<AdminAuctionSummary, ApplicationError> {
        if !command.validate() {
            return Err(DomainError::InvalidSchedule("后台拍卖计划修改命令字段无效").into());
        }
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                let command = command.clone();
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command.command_id).await? {
                        if !stored.matches_request(
                            ADMIN_AUCTION_SCHEDULE_COMMAND_KIND,
                            command.auction_id.as_uuid(),
                            command.request_hash,
                        ) {
                            return Err(CommandLedgerError::Conflict.into());
                        }
                        if stored.result_code != "accepted" {
                            return Err(CompetitionTransactionError::Failed(
                                "Admin 拍卖计划命令账本结果分类不支持重放".to_owned(),
                            )
                            .into());
                        }
                        let replayed: AdminAuctionSummary =
                            serde_json::from_slice(&stored.result_payload)?;
                        if !replayed.validate()
                            || replayed.auction.id != command.auction_id
                            || replayed.auction.preview_scheduled_at != command.preview_scheduled_at
                            || replayed.auction.scheduled_open_at != command.scheduled_open_at
                            || replayed.scheduled_ends_at != Some(command.scheduled_end_at)
                        {
                            return Err(CompetitionTransactionError::Failed(
                                "Admin 拍卖计划命令账本结果无效".to_owned(),
                            )
                            .into());
                        }
                        return Ok(replayed);
                    }

                    let mut snapshot = transaction
                        .load_admin_auction_schedule(command.auction_id)
                        .await?
                        .ok_or(CompetitionTransactionError::NotFound)?;
                    if !snapshot.validate() {
                        return Err(CompetitionTransactionError::Failed(
                            "Admin 拍卖计划快照不一致".to_owned(),
                        )
                        .into());
                    }
                    let auction = &snapshot.summary.auction;
                    if auction.state_version != command.expected_state_version {
                        return Err(CompetitionTransactionError::StaleState.into());
                    }
                    if !matches!(auction.status.as_str(), "draft" | "preview")
                        || auction.scheduled_end_at.is_some()
                        || auction.effective_end_at.is_some()
                        || auction.opened_at.is_some()
                        || snapshot.open_completed_at.is_some()
                        || snapshot.final_completed_at.is_some()
                    {
                        return Err(CompetitionTransactionError::InvalidState.into());
                    }
                    // 必须在取得完整写锁后读取适配器的非冻结权威时钟；否则长时间等锁可能让
                    // 已经到期的结束时间通过校验，并把过早时间写入 updated_at。
                    let now = transaction.current_time().await?;
                    let previous_preview_scheduled_at = auction.preview_scheduled_at;
                    let previous_scheduled_open_at = auction.scheduled_open_at;
                    let previous_scheduled_end_at = snapshot.policy_scheduled_end_at;
                    let changed = previous_preview_scheduled_at != command.preview_scheduled_at
                        || previous_scheduled_open_at != command.scheduled_open_at
                        || previous_scheduled_end_at != command.scheduled_end_at;
                    match auction.status.as_str() {
                        "draft" => {
                            if auction.preview_opened_at.is_some()
                                || snapshot.preview_completed_at.is_some()
                            {
                                return Err(CompetitionTransactionError::InvalidState.into());
                            }
                            if command.preview_scheduled_at <= now
                                || command.scheduled_open_at <= now
                                || command.scheduled_end_at <= now
                            {
                                return Err(CompetitionTransactionError::Conflict.into());
                            }
                        }
                        "preview" => {
                            // 预览已经成为历史事实；这里只允许延后或提前尚未发生的开拍与结束。
                            if auction.preview_opened_at.is_none()
                                || command.preview_scheduled_at != previous_preview_scheduled_at
                                || command.scheduled_open_at <= now
                                || command.scheduled_end_at <= now
                            {
                                return Err(CompetitionTransactionError::Conflict.into());
                            }
                        }
                        _ => return Err(CompetitionTransactionError::InvalidState.into()),
                    }
                    if changed && snapshot.has_active_lease(now) {
                        return Err(CompetitionTransactionError::Conflict.into());
                    }
                    let result = if changed {
                        if let Some(source) = snapshot.schedule_source.as_ref() {
                            snapshot.schedule_source = Some(source.with_updated_times(
                                auction_domain::AuctionScheduleTimes {
                                    preview_due_at: previous_preview_scheduled_at,
                                    open_due_at: previous_scheduled_open_at,
                                    scheduled_end_at: previous_scheduled_end_at,
                                },
                                auction_domain::AuctionScheduleTimes {
                                    preview_due_at: command.preview_scheduled_at,
                                    open_due_at: command.scheduled_open_at,
                                    scheduled_end_at: command.scheduled_end_at,
                                },
                            )?);
                        }
                        transaction
                            .update_admin_auction_schedule(command.clone(), snapshot, now)
                            .await?
                    } else {
                        snapshot.summary
                    };
                    if !result.validate()
                        || result.auction.id != command.auction_id
                        || result.auction.preview_scheduled_at != command.preview_scheduled_at
                        || result.auction.scheduled_open_at != command.scheduled_open_at
                        || result.scheduled_ends_at != Some(command.scheduled_end_at)
                    {
                        return Err(CompetitionTransactionError::Failed(
                            "Admin 拍卖计划修改结果无效".to_owned(),
                        )
                        .into());
                    }
                    transaction
                        .insert_command(StoredCommand {
                            command_id: command.command_id,
                            command_kind: ADMIN_AUCTION_SCHEDULE_COMMAND_KIND.to_owned(),
                            aggregate_id: command.auction_id.as_uuid(),
                            request_hash: command.request_hash,
                            result_code: "accepted".to_owned(),
                            result_payload: serde_json::to_vec(&result)?,
                        })
                        .await?;
                    transaction
                        .append_admin_auction_schedule_record(
                            command,
                            previous_preview_scheduled_at,
                            previous_scheduled_open_at,
                            previous_scheduled_end_at,
                            result.clone(),
                            changed,
                        )
                        .await?;
                    Ok(result)
                })
            })
            .await
    }
}

impl<R> AdminAuctionSchedulePort for AdminAuctionScheduleService<R>
where
    R: TransactionRunner + 'static,
    R::Transaction: CompetitionTransaction + AdminAuctionScheduleTransaction,
{
    fn update_schedule(
        &self,
        command: AdminUpdateAuctionScheduleCommand,
    ) -> ApplicationFuture<'_, AdminAuctionSummary> {
        Box::pin(async move { AdminAuctionScheduleService::update_schedule(self, command).await })
    }
}
