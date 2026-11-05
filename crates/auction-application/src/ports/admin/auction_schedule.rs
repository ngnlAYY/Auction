use std::future::Future;

use auction_domain::{AuctionId, CommandId, valid_text};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{AdminAuctionSummary, ApplicationFuture, CompetitionTransactionError, RequestHash};

/// Admin 拍卖计划修改在命令账本与请求哈希中的稳定种类。
pub const ADMIN_AUCTION_SCHEDULE_COMMAND_KIND: &str = "admin_auction_schedule_update";

#[derive(Debug, Clone)]
pub struct AdminUpdateAuctionScheduleCommand {
    pub command_id: CommandId,
    pub auction_id: AuctionId,
    pub preview_scheduled_at: DateTime<Utc>,
    pub scheduled_open_at: DateTime<Utc>,
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
            && self.preview_scheduled_at.timestamp_micros() != 0
            && self.scheduled_open_at.timestamp_micros() != 0
            && self.scheduled_end_at.timestamp_micros() != 0
            && self.preview_scheduled_at < self.scheduled_open_at
            && self.scheduled_open_at < self.scheduled_end_at
            && self.expected_state_version > 0
            && valid_text(&self.actor_id, 128)
            && self.request_id.as_deref().is_none_or(|value| valid_text(value, 128))
    }
}

/// 锁内读取的计划时间与持久调度租约快照。
#[derive(Debug, Clone)]
pub struct AdminAuctionScheduleSnapshot {
    pub summary: AdminAuctionSummary,
    pub policy_scheduled_end_at: DateTime<Utc>,
    pub round_id: Uuid,
    pub round_state_version: i64,
    pub round_preview_due_at: DateTime<Utc>,
    pub round_open_due_at: DateTime<Utc>,
    pub round_scheduled_end_at: DateTime<Utc>,
    pub schedule_source: Option<auction_domain::AuctionScheduleSource>,
    pub preview_completed_at: Option<DateTime<Utc>>,
    pub open_completed_at: Option<DateTime<Utc>>,
    pub final_completed_at: Option<DateTime<Utc>>,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTime<Utc>>,
}

impl AdminAuctionScheduleSnapshot {
    /// 验证策略快照、调度记录和 Admin 投影来自同一份严格有序计划。
    pub fn validate(&self) -> bool {
        let auction = &self.summary.auction;
        self.summary.validate()
            && !self.round_id.is_nil()
            && self.round_state_version > 0
            && auction.preview_scheduled_at < auction.scheduled_open_at
            && auction.scheduled_open_at < self.policy_scheduled_end_at
            && self.summary.scheduled_ends_at == Some(self.policy_scheduled_end_at)
            && auction.preview_scheduled_at == self.round_preview_due_at
            && auction.scheduled_open_at == self.round_open_due_at
            && self.policy_scheduled_end_at == self.round_scheduled_end_at
            && self.schedule_source.as_ref().is_none_or(|source| source.resolve().is_ok())
            && valid_optional_time(self.preview_completed_at)
            && valid_optional_time(self.open_completed_at)
            && valid_optional_time(self.final_completed_at)
            && self.lease_owner.as_deref().is_none_or(|owner| valid_text(owner, 256))
            && (self.lease_owner.is_none() || self.lease_until.is_some())
            && valid_optional_time(self.lease_until)
    }

    /// 只有同时存在 owner 且尚未到期的 lease 才代表 Worker 正在执行；owner 为空的未来
    /// `lease_until` 是失败退避水位，计划真正改变时可以一并清除。
    pub fn has_active_lease(&self, now: DateTime<Utc>) -> bool {
        self.lease_owner.is_some() && self.lease_until.is_some_and(|until| until > now)
    }
}

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
        previous_preview_scheduled_at: DateTime<Utc>,
        previous_scheduled_open_at: DateTime<Utc>,
        previous_scheduled_end_at: DateTime<Utc>,
        result: AdminAuctionSummary,
        changed: bool,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

fn valid_optional_time(value: Option<DateTime<Utc>>) -> bool {
    value.is_none_or(|value| value.timestamp_micros() != 0)
}
