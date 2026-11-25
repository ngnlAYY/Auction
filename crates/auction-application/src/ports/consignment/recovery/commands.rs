//! 重列命令与回执；撤回沿第 4 期 `WithdrawDepositCommand`，回执在 `control.rs` 扩展。

use auction_domain::consignment::{ListingPrice, ListingState, ListingVersion};
use auction_domain::{CommandId, UserId};
use uuid::Uuid;

use crate::ports::consignment::deposit::{
    DEPOSIT_SCHEMA_VERSION, DepositOperation, ReceiptDisposition,
};
use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::core::RequestHash;

/// 卖家重列：把已止售的原件重新上架为新处分代。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelistCommand {
    /// 玩家 UUIDv4。
    pub command_id: CommandId,
    /// 已认证卖家。
    pub seller_user_id: UserId,
    /// 目标挂单。
    pub listing_id: Uuid,
    /// 已提交挂单版本。
    pub expected_version: ListingVersion,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// 重列回执：`200 relisted`，挂单回到 `available`，带新处分代序号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelistReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 固定 `relist`。
    pub operation: DepositOperation,
    /// 固定 `relisted`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 提交后版本。
    pub listing_version: ListingVersion,
    /// 提交后状态，固定 `Available`。
    pub state: ListingState,
    /// 重列沿用的价格。
    pub price: ListingPrice,
    /// 新处分代序号（上代 + 1）。
    pub generation: u64,
}

impl RelistReceipt {
    /// 构造固定版本、操作、处置与状态的回执；新代序号必须大于 0。
    pub const fn new(
        command_id: CommandId,
        listing_id: Uuid,
        listing_version: ListingVersion,
        price: ListingPrice,
        generation: u64,
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::Relist,
            disposition: ReceiptDisposition::Relisted,
            command_id,
            listing_id,
            listing_version,
            state: ListingState::Available,
            price,
            generation,
        }
    }
}

/// 重列写端口；资格、额度、止售、季界与前代封闭的校验都在写端同一事务内完成。
pub trait CustodyRelistStore: Send + Sync {
    /// 重列。
    fn relist(&self, command: RelistCommand) -> ConsignmentFuture<'_, RelistReceipt>;

    /// 只读重放：按原 `command_id` 返回首次重列结果，不取锁、不写账本。
    ///
    /// 缺失命令返回 `None`；同 ID 异 actor、目标或摘要返回 `CommandIdConflict`；首次是确定拒绝时
    /// 返回同一拒绝。软关闭后仍可调用（不变量 1、D9 §6.3）。
    fn replay_relist(&self, command: RelistCommand)
    -> ConsignmentFuture<'_, Option<RelistReceipt>>;
}
