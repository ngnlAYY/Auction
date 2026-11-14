//! Admin draft lot 修改用例。
//! 在 Serializable 事务中完成命令重放、world/auction/lot 锁内校验、lot 版本更新、
//! 预览修订、withdrawn-return 义务、命令账本、审计和实时事件，避免 HTTP 层复制状态规则。

use std::sync::Arc;

use crate::errors::ApplicationError;
use crate::ports::admin::{
    AdminLotAction, AdminLotSummary, AdminLotUpdateCommand, AdminLotUpdatePort,
    AdminLotUpdateTransaction,
};
use crate::ports::auction::ApplicationFuture;
use crate::ports::core::{
    Clock, CommandLedgerError, CompetitionTransaction, CompetitionTransactionError, StoredCommand,
    TransactionPolicy, TransactionRunner,
};

pub struct AdminLotUpdateService<R, C> {
    repository: Arc<R>,
    clock: Arc<C>,
}

impl<R, C> AdminLotUpdateService<R, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction + AdminLotUpdateTransaction,
    C: Clock,
{
    /// 建立只依赖事务端口和时钟的 lot 修改服务。
    pub fn new(repository: Arc<R>, clock: Arc<C>) -> Self {
        Self { repository, clock }
    }

    /// 执行 draft lot 修改；同价改价保持 lot 与预览版本不变，但仍写入幂等账本和审计。
    pub async fn update(
        &self,
        command: AdminLotUpdateCommand,
    ) -> Result<AdminLotSummary, ApplicationError> {
        command.validate()?;
        let now = self.clock.now();
        // Admin draft lot 修改推进 `lots` 与 `auctions.state_version`，与同场次生命周期迁移
        // 及开拍前后的竞价写入共用一把按场次的门。
        let transaction_policy = TransactionPolicy::serializable();
        self.repository
            .run_in_auction_lot_queue(command.lot_id, transaction_policy, |transaction| {
                let command = command.clone();
                Box::pin(async move {
                    let command_kind = command.action.command_kind();
                    if let Some(stored) =
                        AdminLotUpdateTransaction::find_command(transaction, command.command_id)
                            .await?
                    {
                        if !stored.matches_request(
                            command_kind,
                            command.lot_id.as_uuid(),
                            command.request_hash,
                        ) {
                            return Err(CommandLedgerError::Conflict.into());
                        }
                        if stored.result_code != "accepted" {
                            return Err(CompetitionTransactionError::Failed(
                                "Admin lot 命令账本结果分类不支持重放".to_owned(),
                            )
                            .into());
                        }
                        let replayed: AdminLotSummary =
                            serde_json::from_slice(&stored.result_payload)?;
                        if !replayed.validate() || replayed.id != command.lot_id {
                            return Err(CompetitionTransactionError::Failed(
                                "Admin lot 命令账本结果无效".to_owned(),
                            )
                            .into());
                        }
                        return Ok(replayed);
                    }

                    let mut snapshot = transaction
                        .load_admin_lot_for_update(command.lot_id)
                        .await?
                        .ok_or(CompetitionTransactionError::NotFound)?;
                    if !snapshot.validate() {
                        return Err(CompetitionTransactionError::Failed(
                            "Admin lot 锁内投影无效".to_owned(),
                        )
                        .into());
                    }
                    if snapshot.lot.state_version != command.expected_state_version {
                        return Err(CompetitionTransactionError::StaleState.into());
                    }
                    if snapshot.lot.status != "pending" || snapshot.auction_status != "draft" {
                        return Err(CompetitionTransactionError::InvalidState.into());
                    }
                    if command.action == AdminLotAction::Withdraw && snapshot.lot.seller.is_none() {
                        return Err(CompetitionTransactionError::InvalidState.into());
                    }

                    let changed = match command.action {
                        AdminLotAction::Withdraw => true,
                        AdminLotAction::SetStartPrice => {
                            command.start_price != Some(snapshot.lot.start_price)
                        }
                    };
                    let result = if changed {
                        transaction.update_admin_lot(&command, now).await?
                    } else {
                        snapshot.lot.clone()
                    };
                    let expected_version = command
                        .expected_state_version
                        .checked_add(if changed { 1 } else { 0 })
                        .ok_or_else(|| {
                            CompetitionTransactionError::Failed("Admin lot 状态版本溢出".to_owned())
                        })?;
                    if !result.validate()
                        || result.id != command.lot_id
                        || result.auction_id != snapshot.lot.auction_id
                        || result.state_version != expected_version
                        || match command.action {
                            AdminLotAction::Withdraw => result.status != "withdrawn",
                            AdminLotAction::SetStartPrice => {
                                result.status != "pending"
                                    || result.start_price != command.start_price.unwrap_or_default()
                                    || result.current_price
                                        != command.start_price.unwrap_or_default()
                            }
                        }
                    {
                        return Err(CompetitionTransactionError::Failed(
                            "Admin lot 存储返回结果无效".to_owned(),
                        )
                        .into());
                    }

                    if changed {
                        let next_revision =
                            snapshot.preview_revision.checked_add(1).ok_or_else(|| {
                                CompetitionTransactionError::Failed(
                                    "Admin lot 预览修订溢出".to_owned(),
                                )
                            })?;
                        let expected_auction_state_version =
                            snapshot.auction_state_version.checked_add(1).ok_or_else(|| {
                                CompetitionTransactionError::Failed(
                                    "Admin lot 拍卖状态版本溢出".to_owned(),
                                )
                            })?;
                        let next_auction_state_version = transaction
                            .advance_admin_preview_revision(
                                result.auction_id,
                                snapshot.preview_revision,
                                now,
                            )
                            .await?;
                        if next_auction_state_version != expected_auction_state_version {
                            return Err(CompetitionTransactionError::Failed(
                                "Admin lot 拍卖状态版本推进无效".to_owned(),
                            )
                            .into());
                        }
                        snapshot.preview_revision = next_revision;
                        snapshot.auction_state_version = next_auction_state_version;
                    }

                    AdminLotUpdateTransaction::insert_command(
                        transaction,
                        StoredCommand {
                            command_id: command.command_id,
                            command_kind: command_kind.to_owned(),
                            aggregate_id: command.lot_id.as_uuid(),
                            request_hash: command.request_hash,
                            result_code: "accepted".to_owned(),
                            result_payload: serde_json::to_vec(&result)?,
                        },
                    )
                    .await?;
                    if changed && command.action == AdminLotAction::Withdraw {
                        transaction
                            .insert_withdrawn_return_obligation(&command, &snapshot, now)
                            .await?;
                    }
                    transaction
                        .append_admin_lot_record(
                            &command,
                            &result,
                            changed,
                            snapshot.auction_state_version,
                            snapshot.preview_revision,
                        )
                        .await?;
                    Ok(result)
                })
            })
            .await
    }
}

impl<R, C> AdminLotUpdatePort for AdminLotUpdateService<R, C>
where
    R: TransactionRunner + 'static,
    R::Transaction: CompetitionTransaction + AdminLotUpdateTransaction,
    C: Clock,
{
    fn update(&self, command: AdminLotUpdateCommand) -> ApplicationFuture<'_, AdminLotSummary> {
        Box::pin(async move { AdminLotUpdateService::update(self, command).await })
    }
}
