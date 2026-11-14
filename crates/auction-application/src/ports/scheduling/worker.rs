//! 持久化拍卖轮次的阶段、领取与存储端口。

use auction_domain::{AuctionId, CommandId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ports::auction::ApplicationFuture;
use crate::ports::auction::AuctionPolicyTemplate;

/// 持久化拍卖轮次当前需要执行的生命周期阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduledRoundPhase {
    Preview,
    Open,
    Final,
}

impl ScheduledRoundPhase {
    /// 映射到共享生命周期应用端口的动作；Worker 不复制第二套状态机。
    pub const fn action(self) -> crate::ports::auction::AuctionLifecycleAction {
        match self {
            Self::Preview => crate::ports::auction::AuctionLifecycleAction::Preview,
            Self::Open => crate::ports::auction::AuctionLifecycleAction::Open,
            Self::Final => crate::ports::auction::AuctionLifecycleAction::FinalPhase,
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
    /// 领取成功后的轮次版本；完成/失败必须同时匹配它，避免同一 owner 的旧 lease 回写新一代任务。
    pub state_version: i64,
    pub attempt_count: u32,
}

/// 当前本地周或早鸟场次应存在的持久化轮次及其冻结策略。
/// `round_id`、`auction_id` 和 `create_command_id` 必须由 world/local_week（早鸟场次为
/// world/slot_key）的旧版稳定算法派生；数据库适配器负责在一个事务内幂等建立拍卖、
/// intake target、命令账本和轮次事实。
#[derive(Debug, Clone)]
pub struct ScheduledRoundBootstrap {
    pub round_id: Uuid,
    pub auction_id: AuctionId,
    pub create_command_id: CommandId,
    pub world_id: String,
    pub local_week: chrono::NaiveDate,
    /// 早鸟场次键；常规周轮次为 `None`，并继续受 world/local_week 唯一约束。
    pub slot_key: Option<String>,
    pub preview_due_at: DateTime<Utc>,
    pub open_due_at: DateTime<Utc>,
    pub scheduled_end_at: DateTime<Utc>,
    pub request_hash: crate::ports::core::RequestHash,
    pub policy: AuctionPolicyTemplate,
    /// 新轮次必须冻结原始 IANA 本地计划，后继不按 UTC 固定时长推算。
    pub schedule_source: auction_domain::AuctionScheduleSource,
    /// 新拍卖最早接收装备的时刻；语义与开拍后继一致，不参与请求摘要。
    pub intake_opens_at: Option<DateTime<Utc>>,
}

/// 周计划 ensure 后返回的安全投影，供调度器决定 Mail owner 的暂停/恢复期望。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledRoundSnapshot {
    pub round_id: Uuid,
    pub auction_id: Option<AuctionId>,
    pub active: bool,
    pub world_id: String,
    pub local_week: chrono::NaiveDate,
    /// 早鸟场次键；常规周轮次为 `None`。
    pub slot_key: Option<String>,
    pub preview_due_at: DateTime<Utc>,
    pub open_due_at: DateTime<Utc>,
    pub scheduled_end_at: DateTime<Utc>,
    pub create_command_id: Uuid,
    pub preview_completed_at: Option<DateTime<Utc>>,
    pub open_completed_at: Option<DateTime<Utc>>,
}

/// `claim_due` 的返回值必须来自数据库条件更新/锁定；内存中的 interval 只能唤醒 Worker，
/// 不能成为 due、租约、完成或失败状态的权威来源。
pub trait ScheduledRoundStore: Send + Sync {
    /// 返回数据库权威当前时间；周身份和 due 判断不得使用进程本地时钟。
    fn database_now(&self) -> ApplicationFuture<'_, DateTime<Utc>>;

    /// 只读获取已冻结的常规本地周轮次；配置变化不能作为重发旧创建命令或改写既有策略的理由。
    fn find_current_round(
        &self,
        world_id: String,
        local_week: chrono::NaiveDate,
    ) -> ApplicationFuture<'_, Option<ScheduledRoundSnapshot>>;

    /// 只读获取已冻结的早鸟场次轮次；与常规周轮次的查找键互不重叠。
    fn find_slot_round(
        &self,
        world_id: String,
        slot_key: String,
    ) -> ApplicationFuture<'_, Option<ScheduledRoundSnapshot>>;

    /// 幂等确保当前本地周轮次；若窗口已结束则只建立 missed 轮次，不创建过期拍卖。
    fn ensure_current_round(
        &self,
        bootstrap: ScheduledRoundBootstrap,
    ) -> ApplicationFuture<'_, ScheduledRoundSnapshot>;

    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<ScheduledRoundClaim>>;

    /// 返回严格晚于 `now` 的最早阶段到期或退避租约结束时刻，供 Worker 提前唤醒；
    /// 这只是唤醒提示，是否可领取仍由 `claim_due` 的锁内谓词决定。
    fn next_claimable_at(&self, now: DateTime<Utc>)
    -> ApplicationFuture<'_, Option<DateTime<Utc>>>;

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
