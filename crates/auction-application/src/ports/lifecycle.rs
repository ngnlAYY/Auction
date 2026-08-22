use std::future::Future;

use auction_domain::{Auction, AuctionId, AuctionStatus, CommandId};
use chrono::{DateTime, Utc};
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

    /// 在领域聚合上执行动作，不在 HTTP 层复制状态迁移规则。
    pub fn apply(
        self,
        auction: &mut Auction,
        now: DateTime<Utc>,
    ) -> Result<(), auction_domain::DomainError> {
        match self {
            Self::Preview => auction.preview(now),
            Self::Open => auction.go_live(now),
            Self::Cancel => auction.cancel_with_rules(now, auction_domain::AuctionRules::default()),
            Self::FinalPhase => auction.enter_final(now),
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

/// 生命周期事务只声明读取和更新拍卖聚合所需的窄能力。
pub trait AuctionLifecycleTransaction: Send {
    fn load_auction_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Option<Auction>, CompetitionTransactionError>> + Send;

    fn update_auction(
        &mut self,
        auction: Auction,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;

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
