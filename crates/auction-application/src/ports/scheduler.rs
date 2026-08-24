use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use auction_domain::{AuctionId, CommandId};

use super::ApplicationFuture;

/// 持久化拍卖轮次当前需要执行的生命周期阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduledRoundPhase {
    Preview,
    Open,
    Final,
}

impl ScheduledRoundPhase {
    /// 映射到共享生命周期应用端口的动作；Worker 不复制第二套状态机。
    pub const fn action(self) -> super::AuctionLifecycleAction {
        match self {
            Self::Preview => super::AuctionLifecycleAction::Preview,
            Self::Open => super::AuctionLifecycleAction::Open,
            Self::Final => super::AuctionLifecycleAction::FinalPhase,
        }
    }
}

/// Worker 从持久化轮次表领取的一次阶段任务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledRoundClaim {
    pub round_id: Uuid,
    pub auction_id: AuctionId,
    pub phase: ScheduledRoundPhase,
    pub command_id: CommandId,
    pub worker_id: String,
    pub lease_until: DateTime<Utc>,
    pub attempt_count: u32,
}

/// Worker 调度器使用的窄存储端口。
///
/// `claim_due` 的返回值必须来自数据库条件更新/锁定；内存中的 interval 只能唤醒 Worker，
/// 不能成为 due、租约、完成或失败状态的权威来源。
pub trait ScheduledRoundStore: Send + Sync {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<ScheduledRoundClaim>>;

    fn complete(
        &self,
        claim: ScheduledRoundClaim,
        completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    fn fail(
        &self,
        claim: ScheduledRoundClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()>;
}
