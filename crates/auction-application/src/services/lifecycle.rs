use std::sync::Arc;

use auction_domain::AuctionId;

use crate::{
    ApplicationError, ApplicationFuture, AuctionLifecycleCommand, AuctionLifecycleCommandPort,
    AuctionLifecycleResult, AuctionLifecycleTransaction, CommandLedgerError,
    CompetitionTransaction, CompetitionTransactionError, StoredCommand, TransactionPolicy,
    TransactionRunner,
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

                    let mut auction = transaction
                        .load_auction_for_update(command.auction_id)
                        .await?
                        .ok_or(CompetitionTransactionError::NotFound)?;
                    command.action.apply(&mut auction, now)?;
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
                    transaction.update_auction(auction).await?;
                    transaction.append_lifecycle_record(&command, &result).await?;
                    Ok(result)
                })
            })
            .await
    }
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
