use std::sync::Arc;

use auction_domain::DomainError;

use crate::{
    AdminAuctionCreatePort, AdminAuctionCreateTransaction, AdminAuctionSummary,
    AdminCreateAuctionCommand, ApplicationError, ApplicationFuture, AuctionPolicyTemplate,
    CommandLedgerError, CompetitionTransaction, CompetitionTransactionError, StoredCommand,
    TransactionPolicy, TransactionRunner,
};

/// Admin 创建拍卖用例。
pub struct AdminAuctionCreateService<R> {
    repository: Arc<R>,
    policy: AuctionPolicyTemplate,
}

impl<R> AdminAuctionCreateService<R>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction + AdminAuctionCreateTransaction,
{
    /// 建立只依赖事务端口和冻结策略的创建用例；命令时刻由锁后的事务端口提供。
    pub fn new(repository: Arc<R>, policy: AuctionPolicyTemplate) -> Self {
        Self { repository, policy }
    }

    /// 在同一 Serializable 事务中创建 draft、登记 intake target、账本、审计和创建事件。
    pub async fn create(
        &self,
        command: AdminCreateAuctionCommand,
    ) -> Result<AdminAuctionSummary, ApplicationError> {
        if !command.validate() {
            return Err(DomainError::InvalidSchedule("后台拍卖创建命令字段无效").into());
        }

        let policy = self.policy.clone();
        let title = command.generated_title();
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                let command = command.clone();
                let title = title.clone();
                let policy = policy.clone();
                Box::pin(async move {
                    let command_kind = "admin_auction_create";
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
                                "Admin 拍卖创建命令账本结果分类不支持重放".to_owned(),
                            )
                            .into());
                        }
                        let replayed: AdminAuctionSummary =
                            serde_json::from_slice(&stored.result_payload)?;
                        if !replayed.validate()
                            || replayed.auction.id != command.auction_id
                            || replayed.auction.world_id != command.world_id
                            || replayed.auction.sequence != command.sequence
                        {
                            return Err(CompetitionTransactionError::Failed(
                                "Admin 拍卖创建命令账本结果无效".to_owned(),
                            )
                            .into());
                        }
                        return Ok(replayed);
                    }

                    // 已完成命令不受当前时刻或新模板影响；时间门禁只约束首次创建。
                    // world/intake target 等锁可能跨过预览时间，必须锁齐备后才读取实时数据库时钟。
                    transaction.lock_admin_auction_creation(&command.world_id).await?;
                    let now = transaction.current_time().await?;
                    if command.preview_scheduled_at < now {
                        return Err(
                            DomainError::InvalidSchedule("预览计划时间不能早于当前时间").into()
                        );
                    }
                    policy.validate()?;
                    let result = transaction
                        .create_admin_auction(command.clone(), title, policy, now)
                        .await?;
                    if !result.validate()
                        || result.auction.id != command.auction_id
                        || result.auction.world_id != command.world_id
                        || result.auction.sequence != command.sequence
                    {
                        return Err(CompetitionTransactionError::Failed(
                            "Admin 拍卖创建存储结果无效".to_owned(),
                        )
                        .into());
                    }
                    transaction
                        .insert_command(StoredCommand {
                            command_id: command.command_id,
                            command_kind: command_kind.to_owned(),
                            aggregate_id: command.auction_id.as_uuid(),
                            request_hash: command.request_hash,
                            result_code: "accepted".to_owned(),
                            result_payload: serde_json::to_vec(&result)?,
                        })
                        .await?;
                    transaction
                        .append_admin_auction_created_record(command, result.clone())
                        .await?;
                    Ok(result)
                })
            })
            .await
    }
}

impl<R> AdminAuctionCreatePort for AdminAuctionCreateService<R>
where
    R: TransactionRunner + 'static,
    R::Transaction: CompetitionTransaction + AdminAuctionCreateTransaction,
{
    fn create(
        &self,
        command: AdminCreateAuctionCommand,
    ) -> ApplicationFuture<'_, AdminAuctionSummary> {
        Box::pin(async move { AdminAuctionCreateService::create(self, command).await })
    }
}
