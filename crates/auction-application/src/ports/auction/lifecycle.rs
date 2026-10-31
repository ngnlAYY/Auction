use std::future::Future;

use auction_domain::{Auction, AuctionId, AuctionStatus, CommandId, CompetitionPlan, LotId};
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
    EarlySettlement,
}

impl AuctionLifecycleAction {
    /// 返回命令账本中的稳定命令种类。
    pub const fn command_kind(self) -> &'static str {
        match self {
            Self::Preview => "auction_preview",
            Self::Open => "auction_open",
            Self::Cancel => "auction_cancel",
            Self::FinalPhase => "auction_final_phase",
            Self::EarlySettlement => "auction_early_settlement",
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

/// 进程启动时冻结的生命周期授权策略。
/// Worker 到期开放不依赖该开关；只有 Admin 提前开拍或提前结算需要显式授权。策略随 Admin
/// runtime 文件在重启时生效，不能由单次 HTTP 请求覆盖。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuctionLifecyclePolicy {
    allow_manual_early_open: bool,
    allow_manual_early_settlement: bool,
}

impl AuctionLifecyclePolicy {
    /// 建立不可变的人工生命周期授权策略。
    pub const fn new(allow_manual_early_open: bool, allow_manual_early_settlement: bool) -> Self {
        Self { allow_manual_early_open, allow_manual_early_settlement }
    }

    /// 返回 Admin capability 使用的安全投影。
    pub const fn allows_manual_early_open(self) -> bool {
        self.allow_manual_early_open
    }

    /// 返回人工提前结算 capability 使用的安全投影。
    pub const fn allows_manual_early_settlement(self) -> bool {
        self.allow_manual_early_settlement
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
    /// Cancel 与 EarlySettlement 要求持久化的管理原因；其它动作必须为 `None`。
    pub reason: Option<String>,
    pub request_hash: super::RequestHash,
    /// HTTP 发起时使用请求边界生成/校验的关联 ID；Worker 内部命令为 `None`。
    pub request_id: Option<String>,
    pub actor: AuctionLifecycleActor,
}

/// 生命周期命令提交时冻结的 Admin 拍卖快照。
/// 该快照对应旧版生命周期成功响应的 `data.auction`；它必须与命令结果一起持久化，不能在重放时
/// 另行读取已经继续推进的拍卖投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuctionLifecycleSnapshot {
    pub id: AuctionId,
    pub world_id: String,
    pub sequence: i64,
    pub status: AuctionStatus,
    pub preview_scheduled_at: DateTime<Utc>,
    pub scheduled_open_at: DateTime<Utc>,
    pub preview_opened_at: Option<DateTime<Utc>>,
    pub preview_manual_open: bool,
    pub preview_revision: i64,
    pub opened_at: Option<DateTime<Utc>>,
    pub final_phase_at: Option<DateTime<Utc>>,
    pub settled_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub cancelled_reason: Option<String>,
    pub cancelled_by: Option<String>,
    pub state_version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 生命周期命令提交后的稳定幂等结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuctionLifecycleResult {
    pub command_id: CommandId,
    pub auction_id: AuctionId,
    pub status: AuctionStatus,
    pub state_version: u64,
    pub updated_at: DateTime<Utc>,
    /// `None` 只用于读取扩展快照前已经写入的旧 Rust 账本结果；新命令必须持久化完整快照。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auction: Option<AuctionLifecycleSnapshot>,
}

/// 生命周期事务在锁内读取的拍卖、计划时间和冻结策略上下文。
/// `policy_scheduled_end_at` 来自不可变策略快照，不从可空的运行期列猜测。轮次周仅用于生成
/// 下一期稳定标识；没有轮次的首期拍卖由应用层根据预览日期推导所在周。
#[derive(Debug, Clone)]
pub struct AuctionLifecycleContext {
    pub auction: Auction,
    pub world_id: String,
    /// 与拍卖、可选轮次处于同一锁内快照的世界启用状态。
    pub world_enabled: bool,
    pub sequence: i64,
    pub preview_scheduled_at: DateTime<Utc>,
    pub scheduled_open_at: DateTime<Utc>,
    pub preview_opened_at: Option<DateTime<Utc>>,
    pub policy_scheduled_end_at: DateTime<Utc>,
    pub policy_final_phase_lead: chrono::Duration,
    /// 当前拍卖的冻结结算策略；后继草稿不得继承它来替代有效的新拍卖模板。
    pub policy_settlement: auction_domain::SettlementPolicy,
    pub preview_manual_open: bool,
    pub preview_revision: i64,
    pub opened_at: Option<DateTime<Utc>>,
    pub final_phase_at: Option<DateTime<Utc>>,
    pub settled_at: Option<DateTime<Utc>>,
    pub scheduled_round_local_week: Option<NaiveDate>,
    /// 旧轮次没有来源时保留历史 UTC 七天规则，不推测时区或重写既定时间。
    pub schedule_source: Option<super::AuctionScheduleSource>,
}

/// 当前拍卖结束接收装备时建立的下一期草稿与持久化周轮次。
#[derive(Debug, Clone)]
pub struct AuctionPreviewSuccessorPlan {
    /// 仅应用于本次新建后继的启动期有效模板；不改写前序拍卖。
    pub policy: super::AuctionPolicyTemplate,
    pub schedule_source: Option<super::AuctionScheduleSource>,
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
    /// 新草稿最早接收装备的时刻；早鸟窗口内场次为赛季开始，其余为 `None`。
    /// 由冻结的排期来源与日历确定性派生，随拍卖冻结，不参与创建请求摘要。
    pub intake_opens_at: Option<DateTime<Utc>>,
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
    /// 物化预览，并让当前拍卖继续接收装备。
    Preview { manual: bool },
    /// 激活全部 pending lot、冻结结束时间，并把录入目标切换到下一期草稿。
    Open { manual_override: bool, successor: AuctionPreviewSuccessorPlan },
    /// 取消尚未开拍的拍卖并撤回 pending lot；取消 Preview 时同时建立下一期草稿。
    Cancel { successor: Option<AuctionPreviewSuccessorPlan> },
    /// 进入最终阶段，并原子提交代理转换；人工动作还会把有效截止时间收紧到最终窗口。
    FinalPhase { proxy_plans: Vec<AuctionFinalProxyPlan>, effective_end_at: Option<DateTime<Utc>> },
    /// 立即结束竞价并把拍卖交给既有 settlement Worker；结算事实仍由 Worker 独占写入。
    EarlySettlement {
        proxy_plans: Vec<AuctionFinalProxyPlan>,
        effective_end_at: DateTime<Utc>,
        reason: String,
    },
}

impl AuctionLifecycleEffect {
    /// 返回副作用计划对应的命令动作，供适配器拒绝错配计划。
    pub const fn action(&self) -> AuctionLifecycleAction {
        match self {
            Self::Preview { .. } => AuctionLifecycleAction::Preview,
            Self::Open { .. } => AuctionLifecycleAction::Open,
            Self::Cancel { .. } => AuctionLifecycleAction::Cancel,
            Self::FinalPhase { .. } => AuctionLifecycleAction::FinalPhase,
            Self::EarlySettlement { .. } => AuctionLifecycleAction::EarlySettlement,
        }
    }
}

/// 应用层完成领域校验后交给数据库适配器的原子生命周期计划。
#[derive(Debug, Clone)]
pub struct AuctionLifecyclePlan {
    pub auction: Auction,
    pub effect: AuctionLifecycleEffect,
}

/// 生命周期适配器在原子写入后返回给审计映射的稳定统计。
/// 该统计只描述本次计划实际写入的旧版业务事实，不暴露 SQL 行或后端细节。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuctionLifecycleWriteSummary {
    pub return_obligation_count: u64,
}

/// 生命周期事务只声明锁内上下文与动作计划所需的窄能力。
pub trait AuctionLifecycleTransaction: Send {
    fn load_lifecycle_context_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Option<AuctionLifecycleContext>, CompetitionTransactionError>> + Send;

    /// 在已锁定上下文所在事务内借用计划，原子应用生命周期、副作用和版本 fence。
    fn apply_lifecycle_plan(
        &mut self,
        command: &AuctionLifecycleCommand,
        plan: &AuctionLifecyclePlan,
    ) -> impl Future<Output = Result<AuctionLifecycleWriteSummary, CompetitionTransactionError>> + Send;

    /// 按稳定 lot 顺序锁定并返回 final phase 需要检查的竞价单元。
    fn list_final_lot_ids_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Vec<LotId>, CompetitionTransactionError>> + Send;

    /// 读取 Final 转换使用的锁内 lot 快照；允许截止时间已经到达，但仍要求 world、拍卖和 lot
    /// 处于可收口的 live/final 形状。调用方不得把该入口用于公开竞价。
    fn load_final_lot_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<super::CompetitionSnapshot, CompetitionTransactionError>> + Send;

    /// 在同一事务内追加管理审计和可重放的生命周期事件。
    fn append_lifecycle_record(
        &mut self,
        command: &AuctionLifecycleCommand,
        context: &AuctionLifecycleContext,
        plan: &AuctionLifecyclePlan,
        result: &AuctionLifecycleResult,
        write_summary: AuctionLifecycleWriteSummary,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

/// 生命周期应用端口，供 Admin HTTP 和 Worker 共享。
pub trait AuctionLifecycleCommandPort: Send + Sync {
    /// 返回与命令执行使用同一启动快照的人工提前开放 capability。
    fn allows_manual_early_open(&self) -> bool;

    /// 返回与命令执行使用同一启动快照的人工提前结算 capability。
    fn allows_manual_early_settlement(&self) -> bool;

    fn execute(
        &self,
        command: AuctionLifecycleCommand,
    ) -> ApplicationFuture<'_, AuctionLifecycleResult>;
}
