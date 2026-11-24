//! 卖家用撤回意愿与 Admin 运营止售的代管控制端口。

use auction_domain::CommandId;
use auction_domain::UserId;
use auction_domain::consignment::{HeldOffMarketReason, ListingState, ListingVersion};
use uuid::Uuid;

use crate::ports::consignment::ListingDelistReason;
use crate::ports::consignment::deposit::commands::{
    DEPOSIT_SCHEMA_VERSION, DepositOperation, ReceiptDisposition,
};
use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::core::RequestHash;

/// 卖家对已代管件登记撤回意愿；第 4 期不执行真实 Withdrawal 合同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawDepositCommand {
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

/// 撤回操作的两种处置；wire 码统一由 [`ReceiptDisposition`] 表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WithdrawDisposition {
    /// 代管件登记意愿。
    WithdrawalRequested,
    /// 未代管件关闭。
    Closed,
}

impl WithdrawDisposition {
    /// 稳定 wire 码；与统一回执处置闭集逐字一致。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WithdrawalRequested => "withdrawal_requested",
            Self::Closed => "closed",
        }
    }
}

/// 撤回意愿回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawDepositReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// 处置。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 提交后版本。
    pub listing_version: ListingVersion,
    /// 提交后状态。
    pub state: ListingState,
    /// 意愿 UUID；仅 `withdrawal_requested` 必有。
    pub intent_id: Option<Uuid>,
    /// 第 5 期合同 UUID；第 4 期为空。
    pub contract_id: Option<Uuid>,
    /// 第 5 期装备 UUID；第 4 期为空。
    pub equipment_id: Option<Uuid>,
}

impl WithdrawDepositReceipt {
    /// 构造固定版本与处置的回执。
    pub const fn new(
        command_id: CommandId,
        disposition: WithdrawDisposition,
        listing_id: Uuid,
        listing_version: ListingVersion,
        state: ListingState,
        intent_id: Option<Uuid>,
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::Withdraw,
            disposition: match disposition {
                WithdrawDisposition::WithdrawalRequested => ReceiptDisposition::WithdrawalRequested,
                WithdrawDisposition::Closed => ReceiptDisposition::Closed,
            },
            command_id,
            listing_id,
            listing_version,
            state,
            intent_id,
            contract_id: None,
            equipment_id: None,
        }
    }

    /// 校验回执形状。
    pub fn validate_shape(&self) -> Result<(), auction_domain::consignment::deposit::DepositError> {
        use auction_domain::consignment::deposit::DepositError;
        if self.schema_version != DEPOSIT_SCHEMA_VERSION
            || self.operation != DepositOperation::Withdraw
        {
            return Err(DepositError::InvalidRequest);
        }
        match self.disposition {
            ReceiptDisposition::WithdrawalRequested => {
                if !matches!(
                    self.state,
                    ListingState::DepositAccepted
                        | ListingState::WithdrawalPending
                        | ListingState::HeldOffMarket
                ) || self.intent_id.is_none()
                    || self.contract_id.is_some()
                    || self.equipment_id.is_some()
                {
                    return Err(DepositError::InvalidRequest);
                }
            }
            ReceiptDisposition::Closed => {
                if self.state != ListingState::Closed
                    || self.intent_id.is_some()
                    || self.contract_id.is_some()
                    || self.equipment_id.is_some()
                {
                    return Err(DepositError::InvalidRequest);
                }
            }
            _ => return Err(DepositError::InvalidRequest),
        }
        Ok(())
    }
}

/// Admin 对代管件设运营止售。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetOperationsHoldCommand {
    /// 已认证 Admin 稳定引用。
    pub admin_actor: String,
    /// 玩家 Admin 会话 UUIDv4。
    pub command_id: CommandId,
    /// 目标挂单。
    pub listing_id: Uuid,
    /// 已提交挂单版本。
    pub expected_version: ListingVersion,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 仅审计关联。
    pub request_id: Option<String>,
    /// 私有运营说明，只进私有审计与投影。
    pub reason: ListingDelistReason,
}

/// Admin 解除指定运营止售。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseOperationsHoldCommand {
    /// 已认证 Admin 稳定引用。
    pub admin_actor: String,
    /// 玩家 Admin 会话 UUIDv4。
    pub command_id: CommandId,
    /// 目标挂单。
    pub listing_id: Uuid,
    /// 已提交挂单版本。
    pub expected_version: ListingVersion,
    /// 原止售 UUID。
    pub hold_id: Uuid,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// 止售/下架操作的三种处置；wire 码统一由 [`ReceiptDisposition`] 表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationsHoldDisposition {
    /// 未代管件按前三期规则关闭。
    Closed,
    /// 设置止售。
    HoldSet,
    /// 解除指定止售。
    HoldReleased,
}

impl OperationsHoldDisposition {
    /// 稳定 wire 码；与统一回执处置闭集逐字一致。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::HoldSet => "hold_set",
            Self::HoldReleased => "hold_released",
        }
    }
}

/// 止售回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsHoldReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// 处置。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 提交后版本。
    pub listing_version: ListingVersion,
    /// 提交后状态。
    pub state: ListingState,
    /// 止售 UUID；`closed` 必无，`hold_set` / `hold_released` 必有。
    pub hold_id: Option<Uuid>,
    /// 当前留库原因；解除后固定 `review`。
    pub held_reason: Option<HeldOffMarketReason>,
}

impl OperationsHoldReceipt {
    /// 构造固定版本与处置的回执；构造函数拒绝与处置不符的 hold/状态组合。
    pub fn new(
        command_id: CommandId,
        disposition: OperationsHoldDisposition,
        listing_id: Uuid,
        listing_version: ListingVersion,
        state: ListingState,
        hold_id: Option<Uuid>,
        held_reason: Option<HeldOffMarketReason>,
    ) -> Result<Self, auction_domain::consignment::deposit::DepositError> {
        use auction_domain::consignment::deposit::DepositError;
        let (operation, receipt_disposition) = match disposition {
            OperationsHoldDisposition::Closed => {
                if hold_id.is_some() || state != ListingState::Closed || held_reason.is_some() {
                    return Err(DepositError::InvalidRequest);
                }
                (DepositOperation::AdminDelist, ReceiptDisposition::Closed)
            }
            OperationsHoldDisposition::HoldSet => {
                if hold_id.is_none() || state != ListingState::HeldOffMarket {
                    return Err(DepositError::InvalidRequest);
                }
                (DepositOperation::AdminDelist, ReceiptDisposition::HoldSet)
            }
            OperationsHoldDisposition::HoldReleased => {
                if hold_id.is_none() || state != ListingState::HeldOffMarket {
                    return Err(DepositError::InvalidRequest);
                }
                (DepositOperation::ReleaseOperationsHold, ReceiptDisposition::HoldReleased)
            }
        };
        Ok(Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation,
            disposition: receipt_disposition,
            command_id,
            listing_id,
            listing_version,
            state,
            hold_id,
            held_reason,
        })
    }
}

/// 代管控制端口。
///
/// 库内控制种类 `withdrawal_intent` 对应本端口回执处置 `withdrawal_requested`；前者是
/// `consignment_custody_controls.kind` 的持久事实，后者是 v2 回执的处置码，值不互换。
pub trait CustodyControlStore: Send + Sync {
    /// 卖家登记撤回意愿；`deposit_accepted` 留原态，`available` 转撤回态。
    fn request_withdrawal(
        &self,
        command: WithdrawDepositCommand,
    ) -> ConsignmentFuture<'_, WithdrawDepositReceipt>;

    /// Admin 下架 v2：按锁内状态分流，不按错误回退到 v1。
    ///
    /// `resolving` / `displayed` / `awaiting_deposit` 关闭并写 `admin_delisted`；其中
    /// `awaiting_deposit` 的活动预约随本命令退休。`deposit_accepted` / `available` /
    /// `held_off_market` 设独立运营止售；其余状态返回 `ListingNotActive`。
    fn set_operations_hold(
        &self,
        command: SetOperationsHoldCommand,
    ) -> ConsignmentFuture<'_, OperationsHoldReceipt>;

    /// 只解除指定 hold；解除后保持 `held_off_market/review`，不自动重列。
    fn release_operations_hold(
        &self,
        command: ReleaseOperationsHoldCommand,
    ) -> ConsignmentFuture<'_, OperationsHoldReceipt>;

    /// 只读重放：按原 `command_id` 返回首次 Admin 下架回执。
    ///
    /// 缺失命令返回 `None`，不写账本；同 ID 异 actor、目标或摘要返回 `CommandIdConflict`。
    /// 软关闭后仍可调用本入口重放已接受的命令。
    fn replay_set_operations_hold(
        &self,
        admin_actor: String,
        command_id: CommandId,
        listing_id: Uuid,
        request_hash: RequestHash,
    ) -> ConsignmentFuture<'_, Option<OperationsHoldReceipt>>;
}
