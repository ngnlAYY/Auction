use std::future::Future;

use auction_domain::{
    Auction, AuctionId, AuctionStatus, CommandId, CompetitionPlan, LotId, UserId,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use super::{ApplicationFuture, CompetitionTransactionError};

/// Admin/Worker 可以发起的拍卖生命周期动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuctionLifecycleAction {
    Preview,
    Open,
    Cancel,
    FinalPhase,
}

impl AuctionLifecycleAction {
    /// 返回命令账本中的稳定命令种类。
    pub const fn command_kind(self) -> &'static str {
        match self {
            Self::Preview => "auction_preview",
            Self::Open => "auction_open",
            Self::Cancel => "auction_cancel",
            Self::FinalPhase => "auction_final_phase",
        }
    }
}

/// 生命周期命令的发起者类型；存储层将其映射到稳定的审计枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuctionLifecycleActorType {
    Admin,
    Worker,
    System,
}

impl AuctionLifecycleActorType {
    /// 返回审计表使用的稳定名称；未知类型不能静默降级为另一类身份。
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Worker => "worker",
            Self::System => "system",
        }
    }
}

/// 生命周期命令的发起者；actor ID 进入追加式审计记录，不进入公开响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionLifecycleActor {
    pub actor_type: AuctionLifecycleActorType,
    pub actor_id: String,
}

/// 生命周期命令；同一 `command_id` 的重试必须沿用同一 request hash。
#[derive(Debug, Clone)]
pub struct AuctionLifecycleCommand {
    pub command_id: CommandId,
    pub auction_id: AuctionId,
    pub action: AuctionLifecycleAction,
    pub expected_state_version: Option<u64>,
    pub cancel_reason: Option<String>,
    pub request_hash: super::RequestHash,
    pub actor: AuctionLifecycleActor,
}

/// 生命周期命令提交后的最小稳定投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuctionLifecycleResult {
    pub command_id: CommandId,
    pub auction_id: AuctionId,
    pub status: AuctionStatus,
    pub state_version: u64,
    pub updated_at: DateTime<Utc>,
}

/// 生命周期事务在锁内读取的拍卖、计划时间和冻结策略上下文。
///
/// `policy_scheduled_end_at` 来自不可变策略快照，不从可空的运行期列猜测。轮次周仅用于生成
/// 下一期稳定标识；没有轮次的首期拍卖由应用层根据预览日期推导所在周。
#[derive(Debug, Clone)]
pub struct AuctionLifecycleContext {
    pub auction: Auction,
    pub world_id: String,
    pub sequence: i64,
    pub preview_scheduled_at: DateTime<Utc>,
    pub scheduled_open_at: DateTime<Utc>,
    pub policy_scheduled_end_at: DateTime<Utc>,
    pub policy_final_phase_lead: chrono::Duration,
    pub preview_manual_open: bool,
    pub scheduled_round_local_week: Option<NaiveDate>,
}

/// 开启预览时同时建立的下一期草稿与持久化周轮次。
#[derive(Debug, Clone)]
pub struct AuctionPreviewSuccessorPlan {
    pub round_id: uuid::Uuid,
    pub auction_id: AuctionId,
    pub create_command_id: CommandId,
    pub create_request_hash: super::RequestHash,
    pub local_week: NaiveDate,
    pub sequence: i64,
    pub title: String,
    pub preview_due_at: DateTime<Utc>,
    pub open_due_at: DateTime<Utc>,
    pub scheduled_end_at: DateTime<Utc>,
}

/// Final phase 中单个活动代理的幂等子命令与纯领域竞价计划。
#[derive(Debug, Clone)]
pub struct AuctionFinalProxyPlan {
    pub command: super::StoredCommand,
    pub competition: CompetitionPlan,
}

/// 生命周期动作在领域状态迁移之外必须原子提交的副作用计划。
#[derive(Debug, Clone)]
pub enum AuctionLifecycleEffect {
    /// 物化预览，建立下一期草稿/轮次，并把收件目标转移到下一期。
    Preview { manual: bool, successor: AuctionPreviewSuccessorPlan },
    /// 激活全部 pending lot，并冻结拍卖与 lot 的名义/有效结束时间。
    Open { manual_override: bool },
    /// 取消尚未开拍的拍卖，并撤回其 pending lot。
    Cancel,
    /// 进入最终阶段，并原子提交应用层为每个活动代理计算出的转换计划。
    FinalPhase { proxy_plans: Vec<AuctionFinalProxyPlan> },
}

impl AuctionLifecycleEffect {
    /// 返回副作用计划对应的命令动作，供适配器拒绝错配计划。
    pub const fn action(&self) -> AuctionLifecycleAction {
        match self {
            Self::Preview { .. } => AuctionLifecycleAction::Preview,
            Self::Open { .. } => AuctionLifecycleAction::Open,
            Self::Cancel => AuctionLifecycleAction::Cancel,
            Self::FinalPhase { .. } => AuctionLifecycleAction::FinalPhase,
        }
    }
}

/// 应用层完成领域校验后交给数据库适配器的原子生命周期计划。
#[derive(Debug, Clone)]
pub struct AuctionLifecyclePlan {
    pub auction: Auction,
    pub effect: AuctionLifecycleEffect,
}

/// 生命周期事务只声明锁内上下文与动作计划所需的窄能力。
pub trait AuctionLifecycleTransaction: Send {
    fn load_lifecycle_context_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Option<AuctionLifecycleContext>, CompetitionTransactionError>> + Send;

    /// 在已锁定上下文所在事务内原子应用生命周期、副作用和版本 fence。
    fn apply_lifecycle_plan(
        &mut self,
        command: &AuctionLifecycleCommand,
        plan: AuctionLifecyclePlan,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;

    /// 按稳定 lot 顺序锁定并返回 final phase 需要检查的竞价单元。
    fn list_final_lot_ids_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Vec<LotId>, CompetitionTransactionError>> + Send;

    /// 在同一事务中读取并锁定代理用户资格；失去资格必须冻结，不能静默继续竞价。
    fn proxy_user_is_eligible_for_update(
        &mut self,
        user_id: UserId,
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send;

    /// 在同一事务内追加管理审计和可重放的生命周期事件。
    fn append_lifecycle_record(
        &mut self,
        command: &AuctionLifecycleCommand,
        result: &AuctionLifecycleResult,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

/// 生命周期应用端口，供 Admin HTTP 和 Worker 共享。
pub trait AuctionLifecycleCommandPort: Send + Sync {
    fn execute(
        &self,
        command: AuctionLifecycleCommand,
    ) -> ApplicationFuture<'_, AuctionLifecycleResult>;
}
