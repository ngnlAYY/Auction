//! 寄售处置的冻结清单、工作 claim、证据与写前结果闭集。

use std::fmt;

use auction_domain::SettlementWorld;
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::DispositionAction;

const RETRY_BASE_SECONDS: i64 = 5;
const RETRY_MAXIMUM_SECONDS: i64 = 300;

/// 按 claim 尝试次数计算有界退避的持久到期时刻：5 秒起、每次倍增、上限 5 分钟。
///
/// 不变量 5：到期时间必须由存储持久化，内存定时器只负责唤醒。时刻溢出时取可表示的
/// 最晚时刻，宁可晚领也不提前重试。
pub(crate) fn disposition_retry_at(now: Timestamp, attempt_count: u32) -> Timestamp {
    let exponent = attempt_count.saturating_sub(1).min(6);
    let seconds = RETRY_BASE_SECONDS.saturating_mul(1_i64 << exponent).min(RETRY_MAXIMUM_SECONDS);
    now.checked_add_signed(auction_time::Duration::seconds(seconds)).unwrap_or(Timestamp::MAX_UTC)
}

/// 冻结清单中的一件附件；不含正文或装备 key。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispositionManifestAttachment {
    /// 原序号。
    pub ordinal: u32,
    /// 页面显示名；用于逐项配对系统 marker。
    pub name: String,
    /// 规范 EID。
    pub canonical_eid: Option<String>,
}

/// 4D1 冻结清单的应用层副本；由 4C 持久化、4D1 适配器转换成其严格请求。
#[derive(Clone, PartialEq, Eq)]
pub struct DispositionManifest {
    /// 世界。
    pub world: SettlementWorld,
    /// 远端 MID。
    pub external_mail_id: String,
    /// 完整发件人。
    pub sender: String,
    /// 完整收件人。
    pub recipient: String,
    /// 主题。
    pub subject: String,
    /// 正文摘要。
    pub body_digest: [u8; 32],
    /// Credits。
    pub credits: u64,
    /// COD。
    pub cod: u64,
    /// 原始收件分钟文本。
    pub received_minute_raw: String,
    /// 原始收件时刻。
    pub received_at: Timestamp,
    /// 原序附件。
    pub attachments: Vec<DispositionManifestAttachment>,
    /// 清单摘要；写前逐项比较与领域证明共用。
    pub manifest_digest: [u8; 32],
}

impl DispositionManifest {
    /// 校验可进入处置准备的最小形状；不在此替代 4D1 的逐项预检。
    pub fn validate(&self) -> bool {
        !self.external_mail_id.is_empty()
            && !self.sender.is_empty()
            && !self.recipient.is_empty()
            && !self.subject.is_empty()
            && self.body_digest != [0; 32]
            && self.manifest_digest != [0; 32]
            && self.received_at.timestamp_nanos_opt().is_some()
            && (!self.attachments.is_empty() || self.credits > 0 || self.cod > 0)
    }
}

impl fmt::Debug for DispositionManifest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DispositionManifest")
            .field("world", &self.world)
            .field("external_mail_id", &self.external_mail_id)
            .field("sender", &"[REDACTED]")
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("attachment_count", &self.attachments.len())
            .field("manifest_digest", &self.manifest_digest)
            .finish()
    }
}

/// 已领取的 Take / Return 持久工作及其 fence。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispositionWorkClaim {
    /// 原工作 UUID。
    pub work_id: Uuid,
    /// 最终决定 UUID。
    pub final_decision_id: Uuid,
    /// 投递 UUID。
    pub delivery_id: Uuid,
    /// 资产动作。
    pub action: DispositionAction,
    /// 本次 attempt UUID。
    pub attempt_id: Uuid,
    /// 起写 fence。
    pub fence: u64,
    /// 当前 claim token。
    pub claim_token: Uuid,
    /// 该工作第几次被领取；用于计算重试到期时间。
    pub attempt_count: u32,
    /// 持久租约截止。
    pub lease_until: Timestamp,
    /// 写前冻结清单。
    pub manifest: DispositionManifest,
}

impl DispositionWorkClaim {
    /// 校验存储适配器返回的工作形状。
    pub fn validate(&self) -> bool {
        !self.work_id.is_nil()
            && !self.final_decision_id.is_nil()
            && !self.delivery_id.is_nil()
            && !self.attempt_id.is_nil()
            && !self.claim_token.is_nil()
            && self.fence > 0
            && self.attempt_count > 0
            && self.lease_until.timestamp_nanos_opt().is_some()
            && self.manifest.validate()
    }
}

/// 本地追加的只读证据；不是资产处分权。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispositionObservationEvidence {
    /// 观察身份；证明路径用 4D1 的真实观察 ID，失败路径用由工作与清单派生的本地证据 ID。
    pub observation_id: Uuid,
    /// 观察到的清单摘要。
    pub manifest_digest: [u8; 32],
    /// 认证会话绑定摘要。
    pub session_binding_digest: [u8; 32],
}

/// 核对原因闭集；只进安全日志与持久证据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DispositionReconcileReason {
    /// 写前重读清单与冻结清单不等。
    ManifestChanged,
    /// 身份、世界或凭据代际变化。
    IdentityChanged,
    /// 远端不再允许该动作。
    ActionUnavailable,
    /// 准备的截止已过。
    DeadlineExpired,
    /// 请求形状无效。
    RequestInvalid,
    /// 该工作已有成功起写；本次不得再 POST。
    AlreadyCommitted,
    /// 已起写但确定未发出。
    WriteNotStarted,
    /// 已发出但远端结果未知。
    WriteUnknown,
    /// 已返回但证明不完整或冲突。
    ProofUnproven,
    /// 4D1 证明未签收，不得授予处分权。
    ProofUnsigned,
    /// 起写提交结果未知；只能查账本或核对。
    StartOutcomeUnknown,
    /// 起写被确定拒绝。
    StartRejected,
    /// 证明应用结果不足或冲突。
    EvidenceConflict,
}

impl DispositionReconcileReason {
    /// 稳定安全码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ManifestChanged => "manifest_changed",
            Self::IdentityChanged => "identity_changed",
            Self::ActionUnavailable => "action_unavailable",
            Self::DeadlineExpired => "deadline_expired",
            Self::RequestInvalid => "request_invalid",
            Self::AlreadyCommitted => "already_committed",
            Self::WriteNotStarted => "write_not_started",
            Self::WriteUnknown => "write_unknown",
            Self::ProofUnproven => "proof_unproven",
            Self::ProofUnsigned => "proof_unsigned",
            Self::StartOutcomeUnknown => "start_outcome_unknown",
            Self::StartRejected => "start_rejected",
            Self::EvidenceConflict => "evidence_conflict",
        }
    }
}

/// 可在写前安全重试的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DispositionRetryReason {
    /// 远端临时限流、战斗或会话暂不可用。
    TemporaryUnavailable,
    /// 本地存储暂时不可用；未取得许可。
    StorageUnavailable,
    /// 准备截止已过但尚未起写。
    DeadlineExpired,
}

impl DispositionRetryReason {
    /// 稳定安全码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TemporaryUnavailable => "temporary_unavailable",
            Self::StorageUnavailable => "storage_unavailable",
            Self::DeadlineExpired => "deadline_expired",
        }
    }
}

/// 4D1 写前失败原因的稳定闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsignmentNotSentReason {
    /// 清单变化。
    ManifestChanged,
    /// 身份变化。
    IdentityChanged,
    /// 动作不可用。
    ActionUnavailable,
    /// 截止已过。
    DeadlineExpired,
    /// 许可与目标不匹配。
    PermitMismatch,
    /// POST 尚未开始。
    WriteNotStarted,
    /// 请求形状无效。
    RequestInvalid,
    /// 临时不可用。
    TemporaryUnavailable,
}

impl ConsignmentNotSentReason {
    /// 是否可在写前重试。
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::TemporaryUnavailable | Self::DeadlineExpired)
    }

    /// 映射到核对原因。
    pub const fn reconcile_reason(self) -> DispositionReconcileReason {
        match self {
            Self::ManifestChanged => DispositionReconcileReason::ManifestChanged,
            Self::IdentityChanged => DispositionReconcileReason::IdentityChanged,
            Self::ActionUnavailable => DispositionReconcileReason::ActionUnavailable,
            Self::DeadlineExpired => DispositionReconcileReason::DeadlineExpired,
            Self::PermitMismatch => DispositionReconcileReason::RequestInvalid,
            Self::WriteNotStarted => DispositionReconcileReason::WriteNotStarted,
            Self::RequestInvalid => DispositionReconcileReason::RequestInvalid,
            Self::TemporaryUnavailable => DispositionReconcileReason::WriteNotStarted,
        }
    }
}

/// 4D1 写后待证原因的稳定闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsignmentSentUnprovenReason {
    /// POST 已发出但结果未知。
    WriteUnknown,
    /// POST 成功但证明不完整或冲突。
    ProofUnproven,
}

impl ConsignmentSentUnprovenReason {
    /// 映射到核对原因。
    pub const fn reconcile_reason(self) -> DispositionReconcileReason {
        match self {
            Self::WriteUnknown => DispositionReconcileReason::WriteUnknown,
            Self::ProofUnproven => DispositionReconcileReason::ProofUnproven,
        }
    }
}

/// 4D1 写前失败；观察摘要只在确实重读过页面时提供。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsignmentNotSent {
    /// 安全原因。
    pub reason: ConsignmentNotSentReason,
    /// 本次重读的清单摘要。
    pub observed_manifest_digest: Option<[u8; 32]>,
}

/// 处置请求；适配器负责转换到 4D1 的严格 `DispositionRequest`。
#[derive(Clone, PartialEq, Eq)]
pub struct ConsignmentDispositionRequest {
    /// 动作。
    pub action: DispositionAction,
    /// 最终决定。
    pub final_decision_id: Uuid,
    /// 工作。
    pub work_id: Uuid,
    /// 投递。
    pub delivery_id: Uuid,
    /// attempt。
    pub attempt_id: Uuid,
    /// fence。
    pub fence: u64,
    /// 冻结清单。
    pub manifest: DispositionManifest,
}

impl fmt::Debug for ConsignmentDispositionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsignmentDispositionRequest")
            .field("action", &self.action)
            .field("final_decision_id", &self.final_decision_id)
            .field("work_id", &self.work_id)
            .field("delivery_id", &self.delivery_id)
            .field("attempt_id", &self.attempt_id)
            .field("fence", &self.fence)
            .field("manifest", &self.manifest)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use auction_time::Timestamp;

    use super::disposition_retry_at;

    #[test]
    fn retry_backoff_starts_at_five_seconds_doubles_and_caps_at_five_minutes() {
        let now = Timestamp::UNIX_EPOCH;
        let delays = [1_u32, 2, 3, 4, 5, 6, 7, 8, 100]
            .map(|attempt_count| (disposition_retry_at(now, attempt_count) - now).num_seconds());
        assert_eq!(delays, [5, 10, 20, 40, 80, 160, 300, 300, 300]);
    }
}
