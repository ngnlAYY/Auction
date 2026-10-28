use std::future::Future;

use auction_domain::{AuctionId, CommandId, valid_text};
use chrono::{DateTime, Utc};

use super::{
    AdminAuctionSummary, ApplicationFuture, AuctionPolicyTemplate, CompetitionTransactionError,
    RequestHash,
};

/// 该命令对应旧系统 A03 POST 契约。策略快照由应用服务在事务前按当前已冻结的默认策略
/// 构造并写入，HTTP 请求本身不能注入未审计的数据库字段或外部协议数据。
#[derive(Debug, Clone)]
pub struct AdminCreateAuctionCommand {
    pub command_id: CommandId,
    pub auction_id: AuctionId,
    pub world_id: String,
    pub sequence: i64,
    pub preview_scheduled_at: DateTime<Utc>,
    pub scheduled_open_at: DateTime<Utc>,
    pub scheduled_end_at: DateTime<Utc>,
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

impl AdminCreateAuctionCommand {
    /// 校验不依赖数据库状态的命令字段。
    pub fn validate(&self) -> bool {
        !self.command_id.is_nil()
            && !self.auction_id.is_nil()
            && valid_text(&self.world_id, 64)
            && self.sequence > 0
            && self.expected_state_version == 0
            && valid_time(self.preview_scheduled_at)
            && valid_time(self.scheduled_open_at)
            && valid_time(self.scheduled_end_at)
            && self.preview_scheduled_at < self.scheduled_open_at
            && self.scheduled_end_at > self.scheduled_open_at
            && valid_text(&self.actor_id, 128)
            && self.request_id.as_deref().is_none_or(|value| valid_text(value, 128))
    }

    /// 为当前 schema 仍要求的内部标题生成稳定值；旧 API 没有单独的 title 字段。
    pub fn generated_title(&self) -> String {
        format!("{} #{}", self.world_id, self.sequence)
    }
}

pub trait AdminAuctionCreatePort: Send + Sync {
    fn create(
        &self,
        command: AdminCreateAuctionCommand,
    ) -> ApplicationFuture<'_, AdminAuctionSummary>;
}

/// Admin 创建拍卖事务的 PostgreSQL 完整档窄适配器能力。
pub trait AdminAuctionCreateTransaction: Send {
    /// 按 world → intake target 顺序锁定创建资格；同一事务随后才读取当前时间并执行创建。
    /// 已接受命令先重放，不得为旧结果重新检查世界状态或计划时间。
    fn lock_admin_auction_creation(
        &mut self,
        world_id: &str,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;

    /// 使用本事务已经锁定的创建上下文写入聚合，不再次等待业务锁。
    fn create_admin_auction(
        &mut self,
        command: AdminCreateAuctionCommand,
        title: String,
        policy: AuctionPolicyTemplate,
        created_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminAuctionSummary, CompetitionTransactionError>> + Send;

    fn append_admin_auction_created_record(
        &mut self,
        command: AdminCreateAuctionCommand,
        result: AdminAuctionSummary,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

fn valid_time(value: DateTime<Utc>) -> bool {
    value.timestamp_micros() != 0
}
