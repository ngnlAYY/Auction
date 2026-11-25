//! 自动重列（auto_relist）的端口命令与回执。
//!
//! 只对“未付收回”处置、且决策时冻结的开关为开的处置作自动重列；开关为关的处置留库不动，
//! 之后改配置也不会让它被自动重列。手动重列沿 `CustodyRelistStore`。

use auction_domain::CommandId;
use uuid::Uuid;

use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::consignment::scan::{ScanCursor, ScanPage};
use crate::ports::core::RequestHash;

/// 自动重列命令：命令 ID 由处置 ID 派生。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoRelist {
    /// 系统命令 UUIDv5。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 未付收回处置。
    pub disposition_id: Uuid,
    /// 目标挂单；账本目标。
    pub listing_id: Uuid,
    /// 额度上限；由应用配置冻结值传入，存储只按锁内有效活动数判断，不进摘要。
    pub max_open_per_user: i64,
}

/// 自动重列的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoRelistOutcome {
    /// 已建 g+1 重列代，挂单回到 `available`。
    Relisted {
        /// 挂单。
        listing_id: Uuid,
        /// 提交后挂单版本。
        listing_version: u64,
        /// 新处分代序号。
        generation: u64,
    },
    /// 前置条件未满足，留库不动；每个处置只作一次决定。
    Held {
        /// 挂单。
        listing_id: Uuid,
        /// 挂单当前版本（未变）。
        listing_version: u64,
    },
    /// 不适用或尚未就绪（开关冻结为关、挂单还不是未付收回下架态）：没有写任何行。
    NotReady,
}

/// 自动重列回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoRelistReceipt {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 结果。
    pub outcome: AutoRelistOutcome,
    /// 是否重放。
    pub replayed: bool,
}

/// 待自动重列的处置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingAutoRelist {
    /// 处置。
    pub disposition_id: Uuid,
    /// 挂单。
    pub listing_id: Uuid,
}

/// 自动重列写端。
pub trait AutoRelistStore: Send + Sync {
    /// 锁内重读事实，与手动重列同一判定；通过则建新代并回到 `available`。
    fn auto_relist(&self, command: AutoRelist) -> ConsignmentFuture<'_, AutoRelistReceipt>;

    /// 列出游标之后至多 `limit` 项冻结开关为开、尚无决定、挂单已处于未付收回下架态的处置，按
    /// `(decided_at, 处置 ID)` 升序；读满一页才给出下一页游标。
    fn pending_auto_relists(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<PendingAutoRelist>>;
}
