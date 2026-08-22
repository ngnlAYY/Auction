use std::sync::Arc;

use auction_domain::{AuctionRules, DomainError, RecoveryPolicy, SettlementPolicy};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::{
    AdminAuctionCreatePort, AdminAuctionCreateTransaction, AdminAuctionSummary,
    AdminCreateAuctionCommand, ApplicationError, ApplicationFuture, Clock, CommandLedgerError,
    CompetitionTransaction, CompetitionTransactionError, StoredCommand, TransactionPolicy,
    TransactionRunner,
};

/// Admin 创建拍卖用例。
pub struct AdminAuctionCreateService<R, C> {
    repository: Arc<R>,
    clock: Arc<C>,
}

impl<R, C> AdminAuctionCreateService<R, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction + AdminAuctionCreateTransaction,
    C: Clock,
{
    /// 建立只依赖事务端口和时钟的创建用例。
    pub fn new(repository: Arc<R>, clock: Arc<C>) -> Self {
        Self { repository, clock }
    }

    /// 在同一 Serializable 事务中创建 draft、登记 intake target、账本、审计和创建事件。
    pub async fn create(
        &self,
        command: AdminCreateAuctionCommand,
    ) -> Result<AdminAuctionSummary, ApplicationError> {
        if !command.validate() {
            return Err(DomainError::InvalidSchedule("后台拍卖创建命令字段无效").into());
        }
        let now = self.clock.now();
        if command.preview_scheduled_at < now {
            return Err(DomainError::InvalidSchedule("预览计划时间不能早于当前时间").into());
        }

        let policy_snapshot = policy_snapshot(command.scheduled_end_at);
        let title = command.generated_title();
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                let command = command.clone();
                let title = title.clone();
                let policy_snapshot = policy_snapshot.clone();
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

                    let result = transaction
                        .create_admin_auction(command.clone(), title, policy_snapshot, now)
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

impl<R, C> AdminAuctionCreatePort for AdminAuctionCreateService<R, C>
where
    R: TransactionRunner + 'static,
    R::Transaction: CompetitionTransaction + AdminAuctionCreateTransaction,
    C: Clock,
{
    fn create(
        &self,
        command: AdminCreateAuctionCommand,
    ) -> ApplicationFuture<'_, AdminAuctionSummary> {
        Box::pin(async move { AdminAuctionCreateService::create(self, command).await })
    }
}

/// 生成当前迁移切片冻结的策略快照。
///
/// 策略表/规则管理端尚未迁移前，创建命令只能使用这一份显式默认值；把它写入拍卖而不是
/// 在竞价时实时读取，保证后续规则管理不会改变已经创建的轮次。
fn policy_snapshot(scheduled_end_at: DateTime<Utc>) -> Value {
    let rules = AuctionRules::default();
    let settlement = SettlementPolicy::default();
    let recovery = RecoveryPolicy::default();
    json!({
        "schema_version": 4,
        "start_price": rules.default_start_price.value(),
        "bid_increment_ppm": 0,
        "minimum_increment": 30_000,
        "soft_close_window_seconds": rules.soft_close_window.num_seconds(),
        "soft_close_extension_seconds": rules.soft_close_extension.num_seconds(),
        "proxy_enabled": true,
        "proxy_default_mode": "fast",
        "proxy_slow_interval_seconds": rules.proxy_slow_interval.num_seconds(),
        "proxy_max_outbid_uses": 0,
        "final_phase_lead_seconds": 3_600,
        "scheduled_end_at": scheduled_end_at.to_rfc3339(),
        "settlement": {
            "version": settlement.version,
            "mode": "hold_curve_v1",
            "compensation_pool_enabled": settlement.compensation_pool_enabled,
            "isekai_carrier_credit": settlement.isekai_carrier_credit,
            "anchors": settlement.anchors,
        },
        "recovery": {
            "version": recovery.version,
            "mode": "remind_v1",
            "payment_timeout_seconds": recovery.payment_timeout.num_seconds(),
            "reminder_limit": recovery.reminder_limit,
        },
    })
}
