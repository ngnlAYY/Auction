//! 季前撤回通知与旧季关闭的端口命令与回执。

use auction_domain::CommandId;
use auction_domain::SettlementWorld;
use auction_domain::UserId;
use auction_domain::consignment::FrozenSeason;
use uuid::Uuid;

use crate::ports::consignment::deposit::commands::{
    DEPOSIT_SCHEMA_VERSION, DepositOperation, ReceiptDisposition,
};
use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::core::RequestHash;

/// 季前通知种类；第 4 期只产生撤回通知。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeasonNoticeKind {
    /// 提示卖家可在季末前撤回。
    PrecloseWithdrawal,
}

impl SeasonNoticeKind {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrecloseWithdrawal => "preclose_withdrawal",
        }
    }
}

/// 按 `(season, position, seller)` 去重的季前通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonNoticeCommand {
    /// 应用层派生的 UUIDv5。
    pub command_id: CommandId,
    /// 冻结季窗。
    pub season: FrozenSeason,
    /// position UUID。
    pub position_id: Uuid,
    /// 卖家。
    pub seller_user_id: UserId,
    /// 通知种类。
    pub notice_kind: SeasonNoticeKind,
    /// 规范摘要。
    pub request_hash: RequestHash,
}

/// 通知排队回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonNoticeReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// `notice_queued`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 季身份。
    pub season_id: String,
    /// position UUID。
    pub position_id: Uuid,
    /// 卖家。
    pub seller_user_id: UserId,
    /// 通知工作 UUID。
    pub work_id: Uuid,
}

impl SeasonNoticeReceipt {
    /// 构造固定版本与处置的成功回执。
    pub fn queued(
        command_id: CommandId,
        season_id: String,
        position_id: Uuid,
        seller_user_id: UserId,
        work_id: Uuid,
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::SeasonNotice,
            disposition: ReceiptDisposition::NoticeQueued,
            command_id,
            season_id,
            position_id,
            seller_user_id,
            work_id,
        }
    }
}

/// 旧季关闭的目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeasonCloseTarget {
    /// 关闭该世界的旧季市场。
    World(SettlementWorld),
    /// 关闭指定挂单。
    Listing(Uuid),
    /// 关闭指定 position 对应的挂单。
    Position(Uuid),
}

impl SeasonCloseTarget {
    /// 稳定 wire 码。
    pub const fn kind(self) -> &'static str {
        match self {
            Self::World(_) => "world",
            Self::Listing(_) => "listing",
            Self::Position(_) => "position",
        }
    }

    /// 目标 UUID；world 目标没有 UUID。
    pub const fn id(self) -> Option<Uuid> {
        match self {
            Self::World(_) => None,
            Self::Listing(id) | Self::Position(id) => Some(id),
        }
    }
}

/// 关闭旧季；目标与冻结界都进入摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonCloseCommand {
    /// 应用层派生的 UUIDv5。
    pub command_id: CommandId,
    /// 冻结季窗。
    pub season: FrozenSeason,
    /// 关闭目标。
    pub target: SeasonCloseTarget,
    /// 规范摘要。
    pub request_hash: RequestHash,
}

/// 旧季关闭回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonCloseReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// `season_closed`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 季身份。
    pub season_id: String,
    /// 关闭目标。
    pub target: SeasonCloseTarget,
    /// 本次关闭的挂单。
    pub closed_listing_ids: Vec<Uuid>,
}

impl SeasonCloseReceipt {
    /// 构造固定版本与处置的成功回执。
    pub fn closed(
        command_id: CommandId,
        season_id: String,
        target: SeasonCloseTarget,
        closed_listing_ids: Vec<Uuid>,
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::SeasonClose,
            disposition: ReceiptDisposition::SeasonClosed,
            command_id,
            season_id,
            target,
            closed_listing_ids,
        }
    }
}

/// 季节任务端口。
pub trait SeasonStore: Send + Sync {
    /// 预停后通知每个卖家；通知去重覆盖预停后新取得的件。
    fn queue_season_notice(
        &self,
        command: SeasonNoticeCommand,
    ) -> ConsignmentFuture<'_, SeasonNoticeReceipt>;

    /// 关闭旧季目标；迟跑任务不能延长原冻结界。
    fn close_season(
        &self,
        command: SeasonCloseCommand,
    ) -> ConsignmentFuture<'_, SeasonCloseReceipt>;
}
