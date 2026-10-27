use chrono::{DateTime, Utc};
use uuid::Uuid;

use auction_domain::{AuctionId, CommandId};

use super::ApplicationFuture;

/// Worker 领取的一次持久化结算执行任务。
/// `settlement_command_id` 按 workflow 稳定派生；Worker 重启或租约过期后必须继续使用同一个
/// 命令，而不是为同一拍卖创建新的结算事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementWorkClaim {
    pub auction_id: AuctionId,
    pub workflow_id: Uuid,
    pub claim_command_id: CommandId,
    pub settlement_command_id: CommandId,
    /// 领取时冻结的最晚 lot 有效结束时间；重试必须保持相同世代。
    pub due_at: DateTime<Utc>,
    /// 仅补写结算完成 fence，不再次调用结算用例。
    pub completion_only: bool,
    pub worker_id: String,
    pub lease_until: DateTime<Utc>,
    pub attempt_count: u32,
}

/// 持久化结算调度端口。
/// 领取、租约、失败退避和完成标记由数据库字段决定，不以易失内存任务替代。
pub trait SettlementWorkStore: Send + Sync {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<SettlementWorkClaim>>;

    /// 返回严格晚于 `now` 的最早结算到期或重试时刻，只作 Worker 唤醒提示，不放宽领取谓词。
    fn next_claimable_at(&self, now: DateTime<Utc>)
    -> ApplicationFuture<'_, Option<DateTime<Utc>>>;

    fn complete(
        &self,
        claim: SettlementWorkClaim,
        completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    fn fail(
        &self,
        claim: SettlementWorkClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()>;
}
