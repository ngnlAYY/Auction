//! 派发端口的命令与回执。

use auction_domain::CommandId;
use auction_domain::consignment::dispatch::{
    AttemptKey, DispatchRoundKey, EvidenceCategory, FrozenDispatchIdentity, RoundOrigin,
};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::core::RequestHash;
use crate::ports::delivery::DeliverySendCredentialBinding;

/// 派发端口的操作闭集；账本 `operation` 码，目标种类见各命令。
///
/// 与 `DepositOperation::ALL` 的并集由集成人在迁移 136 的 CHECK 与回执校验函数里对账。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DispatchOperation {
    /// 授权首轮或合法新轮。
    AuthorizeDispatch,
    /// 登记并冻结一次尝试。
    StartAttempt,
    /// 提交 `write_started`。
    MarkWriteStarted,
    /// 追加证据。
    RecordEvidence,
    /// 证明买家已确领原件（原件腿），同事务放行卖款。
    ProveHandoff,
}

impl DispatchOperation {
    /// 稳定账本码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthorizeDispatch => "authorize_dispatch",
            Self::StartAttempt => "start_attempt",
            Self::MarkWriteStarted => "mark_write_started",
            Self::RecordEvidence => "record_evidence",
            Self::ProveHandoff => "prove_handoff",
        }
    }
}

/// 建首轮或合法新轮。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeDispatch {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 轮业务键。
    pub key: DispatchRoundKey,
    /// 来源；新轮必须引用已封闭上一轮的接管。
    pub origin: RoundOrigin,
    /// 本轮的投递。
    pub delivery_id: Uuid,
    /// 冻结身份。
    pub frozen: FrozenDispatchIdentity,
}

/// 授权回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundReceipt {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 轮 ID。
    pub round_id: Uuid,
    /// 轮次号。
    pub round: u32,
    /// 投递。
    pub delivery_id: Uuid,
    /// 是否重放。
    pub replayed: bool,
}

/// 登记一次尝试；冻结身份必须与轮一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartAttempt {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 轮 ID。
    pub round_id: Uuid,
    /// 尝试键。
    pub attempt: AttemptKey,
    /// 当前 claim token。
    pub claim_token: Uuid,
    /// 冻结身份。
    pub frozen: FrozenDispatchIdentity,
}

/// 尝试回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptReceipt {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 尝试 ID。
    pub attempt_id: Uuid,
    /// 是否重放。
    pub replayed: bool,
}

/// 提交起写；只有首次提交的执行者可 POST。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkWriteStarted {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 尝试键。
    pub attempt: AttemptKey,
    /// 当前 claim token。
    pub claim_token: Uuid,
    /// 发送所用的 active 凭据绑定；起写事务锁验它并写入冻结的 Send 身份，`None` 不锁凭据（与拍卖一致）。
    pub credential: Option<DeliverySendCredentialBinding>,
}

/// 追加一条证据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordEvidence {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 尝试键。
    pub attempt: AttemptKey,
    /// 证据类别。
    pub category: EvidenceCategory,
    /// 远端观察 ID。
    pub remote_observation_id: String,
    /// 观察时刻。
    pub observed_at: Timestamp,
    /// 产出该证据的验证器版本；持久列 `validator_version` 非空，不由存储端臆造。
    pub validator_version: String,
    /// 证据摘要。
    pub evidence_digest: [u8; 32],
}

/// 证据回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvidenceReceipt {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 是否重放（同键证据已存在）。
    pub replayed: bool,
}

/// 证明买家已确领原件（D9 §2.3 `recipient_took_original`、§6.2 `prove_handoff`）：main 的原件 COD 腿或
/// isekai 的原件交割腿。业务键为原物义务加原轮；所消费的证据身份与摘要进请求摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProveHandoff {
    /// 命令 ID（应用层按原物义务加原轮派生的 UUIDv5）。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 原件腿的轮业务键。
    pub key: DispatchRoundKey,
    /// 该轮已起写的尝试。
    pub attempt: AttemptKey,
    /// 确领证据（`original_claim` 类）的远端观察 ID。
    pub remote_observation_id: String,
    /// 确领证据摘要；必须与已存证据逐字节相同。
    pub evidence_digest: [u8; 32],
}

/// 确领回执：确领证明与成交关闭后的挂单版本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffReceipt {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 确领证明 ID。
    pub proof_id: Uuid,
    /// 挂单。
    pub listing_id: Uuid,
    /// 成交关闭后的挂单版本。
    pub listing_version: u64,
    /// 是否重放。
    pub replayed: bool,
}
