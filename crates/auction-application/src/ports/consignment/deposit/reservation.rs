//! 预约签发、换发与退休的端口命令与回执。

use auction_domain::CommandId;
use auction_domain::UserId;
use auction_domain::consignment::{ListingState, ListingVersion};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::deposit::commands::{
    DEPOSIT_SCHEMA_VERSION, DepositOperation, ReceiptDisposition,
};
use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::core::RequestHash;

/// 本人签发或换发预约；新码与新预约 ID 是首次执行结果，不能使重放换码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueReservationCommand {
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
    /// 仅审计关联，不参与摘要。
    pub request_id: Option<String>,
}

/// `201 reservation_issued` 回执；不携带令牌正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// `reservation_issued`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 提交后的挂单版本。
    pub listing_version: ListingVersion,
    /// 提交后的挂单状态。
    pub state: ListingState,
    /// 预约 UUID。
    pub reservation_id: Uuid,
    /// 预约截止。
    pub expires_at: Timestamp,
}

impl ReservationReceipt {
    /// 构造固定版本与处置的成功回执。
    pub fn issued(
        command_id: CommandId,
        listing_id: Uuid,
        listing_version: ListingVersion,
        state: ListingState,
        reservation_id: Uuid,
        expires_at: Timestamp,
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::IssueReservation,
            disposition: ReceiptDisposition::ReservationIssued,
            command_id,
            listing_id,
            listing_version,
            state,
            reservation_id,
            expires_at,
        }
    }
}

/// 预约失效事实；扫描时刻本身不能作为失效事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationInvalidation {
    /// 到达签发时冻结的截止。
    Expired {
        /// 原截止时刻。
        expires_at: Timestamp,
    },
    /// 卖家取消命令。
    Cancelled {
        /// 取消命令 UUID。
        command_id: CommandId,
    },
    /// 身份失效事实。
    IdentityLost {
        /// 稳定证据 UUID。
        fact_id: Uuid,
    },
    /// 服务停用代次。
    ServiceDisabled {
        /// 递增后的服务代次。
        service_epoch: u64,
    },
}

impl ReservationInvalidation {
    /// 稳定 wire 码。
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Expired { .. } => "expired",
            Self::Cancelled { .. } => "cancelled",
            Self::IdentityLost { .. } => "identity_lost",
            Self::ServiceDisabled { .. } => "service_disabled",
        }
    }
}

/// Worker 或命令子事实退休预约。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpireReservationCommand {
    /// 应用层派生的 UUIDv5。
    pub command_id: CommandId,
    /// 预约 UUID。
    pub reservation_id: Uuid,
    /// 预约代。
    pub generation: u64,
    /// 失效事实。
    pub invalidation: ReservationInvalidation,
    /// 规范摘要。
    pub request_hash: RequestHash,
}

/// `reservation_retired` 回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRetired {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// `reservation_retired`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 预约 UUID。
    pub reservation_id: Uuid,
}

impl ReservationRetired {
    /// 构造固定版本与处置的成功回执。
    pub const fn new(command_id: CommandId, reservation_id: Uuid) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::ExpireReservation,
            disposition: ReceiptDisposition::ReservationRetired,
            command_id,
            reservation_id,
        }
    }
}

/// 预约端口。实现方须在同一事务内撤旧签新、冻结代与截止并追加事件。
pub trait ReservationStore: Send + Sync {
    /// 仅向本人已验证的 `awaiting_deposit` 签发；换发原子撤销旧码，改价不换码。
    /// 换发时旧预约以 `retired/reissued` 退休。
    ///
    /// 不变量 1、2、3、6、7：预约码只以摘要持久化，回执不带秘密。
    fn issue_reservation(
        &self,
        command: IssueReservationCommand,
    ) -> ConsignmentFuture<'_, ReservationReceipt>;

    /// 按预约、代与失效事实退休；不能退休后继。
    fn expire_reservation(
        &self,
        command: ExpireReservationCommand,
    ) -> ConsignmentFuture<'_, ReservationRetired>;

    /// 只读重放：按原 `command_id` 返回首次预约签发回执。
    ///
    /// 缺失命令返回 `None`，不写账本；同 ID 异 actor、目标或摘要返回 `CommandIdConflict`。
    /// 软关闭后仍可调用本入口重放已接受的命令。
    fn replay_issue_reservation(
        &self,
        seller_user_id: UserId,
        command_id: CommandId,
        listing_id: Uuid,
        request_hash: RequestHash,
    ) -> ConsignmentFuture<'_, Option<ReservationReceipt>>;
}
