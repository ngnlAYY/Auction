//! 退件接管端口的命令与回执。

use auction_domain::CommandId;
use auction_domain::consignment::returns::{ReturnIntakeFacts, ReturnLeg};
use uuid::Uuid;

use crate::ports::core::RequestHash;

/// 退件接管端口的操作闭集；账本 `operation` 码。
///
/// 起写在账本里是 `start_return_take`，回执沿用第 4 期 `StartReceipt`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReturnOperation {
    /// 按 RTS 的 K 建 intake、Take 工作与终局。
    AcquireReturn,
    /// 提交 Take 起写。
    StartReturnTake,
    /// 证明 + 消费 + 资金回补。
    ConsumeReturn,
}

impl ReturnOperation {
    /// 稳定账本码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AcquireReturn => "acquire_return",
            Self::StartReturnTake => "start_return_take",
            Self::ConsumeReturn => "consume_return",
        }
    }
}

/// 接管命令：应用层带上它判定过的事实，存储在锁内重读并以同一规则复判，不信任调用方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquireReturn {
    /// 命令 ID（目标是 RTS 的 K）。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 应用层判定的接管事实。
    pub facts: ReturnIntakeFacts,
}

/// 接管回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquireReceipt {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 接管 ID。
    pub intake_id: Uuid,
    /// 唯一 Take 工作。
    pub work_id: Uuid,
    /// 原信最终处置 ID。
    pub decision_id: Uuid,
    /// 是否重放。
    pub replayed: bool,
}

/// Take 起写命令；命令 ID 由 `start_return_take_command_id` 按 `(intake, work, attempt, fence)` 派生。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartReturnTake {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 接管 ID。
    pub intake_id: Uuid,
    /// 唯一 Take 工作。
    pub work_id: Uuid,
    /// 该工作的 Take 投递；存储核对它就是工作绑定的投递，应用层据此铸造许可。
    pub delivery_id: Uuid,
    /// 本次 attempt。
    pub attempt_id: Uuid,
    /// 当前 fence。
    pub fence: u64,
    /// 当前 claim token。
    pub claim_token: Uuid,
    /// 证据会话绑定摘要。
    pub session_binding_digest: [u8; 32],
    /// 写前冻结清单摘要。
    pub expected_manifest_digest: [u8; 32],
}

/// 消费命令：引用原起写 attempt 与精确 Take 证明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumeReturn {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 接管 ID。
    pub intake_id: Uuid,
    /// 原起写 attempt。
    pub original_attempt_id: Uuid,
    /// 原起写 fence。
    pub fence: u64,
    /// 证明观察。
    pub observation_id: Uuid,
    /// 验证器版本。
    pub validator_version: String,
    /// 证明摘要。
    pub proof_digest: [u8; 32],
}

/// 消费回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsumeReceipt {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 接管 ID。
    pub intake_id: Uuid,
    /// 证明 ID。
    pub proof_id: Uuid,
    /// 退回腿。
    pub leg: ReturnLeg,
    /// 是否重放。
    pub replayed: bool,
}
