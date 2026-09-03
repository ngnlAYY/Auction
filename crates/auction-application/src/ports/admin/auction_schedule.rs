use std::future::Future;

use auction_domain::{AuctionId, CommandId};
use chrono::{DateTime, Utc};

use super::{AdminAuctionSummary, ApplicationFuture, CompetitionTransactionError, RequestHash};

/// Admin 拍卖计划修改在命令账本与请求哈希中的稳定种类。
pub const ADMIN_AUCTION_SCHEDULE_COMMAND_KIND: &str = "admin_auction_schedule_update";

/// Admin 在正式开拍前修改计划结束时间的命令。
#[derive(Debug, Clone)]
pub struct AdminUpdateAuctionScheduleCommand {
    pub command_id: CommandId,
    pub auction_id: AuctionId,
    pub scheduled_end_at: DateTime<Utc>,
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

impl AdminUpdateAuctionScheduleCommand {
    /// 校验不依赖数据库状态的命令字段。
    pub fn validate(&self) -> bool {
        !self.command_id.is_nil()
            && !self.auction_id.is_nil()
            && self.scheduled_end_at.timestamp_micros() != 0
            && self.expected_state_version > 0
            && valid_text(&self.actor_id, 128)
            && self.request_id.as_deref().is_none_or(|value| valid_text(value, 128))
    }
}

/// 锁内读取的计划结束时间快照。
#[derive(Debug, Clone)]
pub struct AdminAuctionScheduleSnapshot {
    pub summary: AdminAuctionSummary,
    pub policy_scheduled_end_at: DateTime<Utc>,
    pub round_scheduled_end_at: DateTime<Utc>,
}

impl AdminAuctionScheduleSnapshot {
    /// 验证策略快照、调度记录和 Admin 投影来自同一份计划。
    pub fn validate(&self) -> bool {
        self.summary.validate()
            && self.summary.scheduled_ends_at == Some(self.policy_scheduled_end_at)
            && self.policy_scheduled_end_at == self.round_scheduled_end_at
    }
}

/// Admin 拍卖计划修改的窄应用端口。
pub trait AdminAuctionSchedulePort: Send + Sync {
    fn update_schedule(
        &self,
        command: AdminUpdateAuctionScheduleCommand,
    ) -> ApplicationFuture<'_, AdminAuctionSummary>;
}

/// Admin 拍卖计划修改的 PostgreSQL 完整档事务能力。
pub trait AdminAuctionScheduleTransaction: Send {
    fn load_admin_auction_schedule(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<
        Output = Result<Option<AdminAuctionScheduleSnapshot>, CompetitionTransactionError>,
    > + Send;

    fn update_admin_auction_schedule(
        &mut self,
        command: AdminUpdateAuctionScheduleCommand,
        snapshot: AdminAuctionScheduleSnapshot,
        updated_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminAuctionSummary, CompetitionTransactionError>> + Send;

    fn append_admin_auction_schedule_record(
        &mut self,
        command: AdminUpdateAuctionScheduleCommand,
        previous_scheduled_end_at: DateTime<Utc>,
        result: AdminAuctionSummary,
        changed: bool,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}
