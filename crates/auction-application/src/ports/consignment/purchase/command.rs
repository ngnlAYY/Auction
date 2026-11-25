//! 购买与 v2 改价的端口命令；摘要由应用层按规范编码计算，存储只比较不重算。

use auction_domain::consignment::purchase::{PurchaseInput, RepriceInput};
use auction_domain::{CommandId, UserId};

use crate::ports::core::RequestHash;

/// 买家购买一件 `available` 挂单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseCommand {
    /// 玩家 UUIDv4。
    pub command_id: CommandId,
    /// `canonical_purchase` 的 SHA-256；同 ID 同摘要返回原结果，同 ID 异摘要冲突。
    pub request_hash: RequestHash,
    pub buyer_user_id: UserId,
    pub input: PurchaseInput,
    /// 仅审计关联，不入摘要。
    pub request_id: Option<String>,
}

/// 卖家对 `available` 挂单改价（R5-C11，schema 2）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepriceV2Command {
    /// 玩家 UUIDv4。
    pub command_id: CommandId,
    pub request_hash: RequestHash,
    pub seller_user_id: UserId,
    pub input: RepriceInput,
    /// 仅审计关联，不入摘要。
    pub request_id: Option<String>,
}
