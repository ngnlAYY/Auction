//! MoogleMail 周期观察与恢复：Inbox 由持久 intake journal 处理，Read Mail 仅观察。
//!
//! 新 Send 只凭原尝试冻结的主题、正文摘要、世界、收件人和完整资产唯一恢复；
//! 旧行保留原有保守证明，不能以当前登录 key 重建历史发送。未找到 Sent 不证明未发送，
//! 未知 Send 永不自动重试。compose 草稿仍只在精确匹配后持短时独占锁受控丢弃。
//!
//! 子模块分工：`sweep` 负责单轮候选轮转与预算，`sent` 负责 Sent/Take 恢复与 Read Mail
//! 观察，`cleanup` 负责精确匹配草稿的受控丢弃，`diagnostics` 负责预算错误与远端读取警告。

mod cleanup;
mod diagnostics;
mod disposition;
mod frozen_manifest;
mod matching;
mod quarantine_recheck;
mod sent;
mod sweep;
mod write_transport;

pub use disposition::{
    DispositionMaintenanceService, DispositionRound, DispositionRoundRequest, DispositionWorkStore,
};
pub use frozen_manifest::{
    ConsignmentDispositionRequest, ConsignmentNotSent, ConsignmentNotSentReason,
    ConsignmentSentUnprovenReason, DispositionManifest, DispositionManifestAttachment,
    DispositionObservationEvidence, DispositionReconcileReason, DispositionRetryReason,
    DispositionWorkClaim,
};
pub use quarantine_recheck::{
    QuarantineMailLocator, QuarantineRecheckClaim, QuarantineRecheckReadError,
    QuarantineRecheckReader, QuarantineRecheckRound, QuarantineRecheckService,
    QuarantineRecheckStore, QuarantineReobservation,
};
pub use write_transport::{
    ConsignmentDispositionExecution, ConsignmentDispositionProof, ConsignmentDispositionTransport,
    PreparedDisposition, SignedConsignmentProof, disposition_proof_observation_id,
};

use std::sync::Arc;

use uuid::Uuid;

use crate::ports::core::Clock;
use crate::ports::mail::{MoogleMailboxEvidenceStore, MoogleMailboxMaintenance};

/// compose 清理中断的阶段；远端失败的安全分类已由诊断记录，报告不携带错误原文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeCleanupFailure {
    /// 读取 compose 草稿失败。
    ReadCompose,
    /// 丢弃已尝试或投递状态已变化，围栏不授权 POST，保留人工核对。
    FenceDenied,
    /// 围栏提交后的 discard 失败；不会自动重新授权。
    DiscardCompose,
}

impl ComposeCleanupFailure {
    /// 日志字段使用的稳定标签。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadCompose => "read_compose",
            Self::FenceDenied => "fence_denied",
            Self::DiscardCompose => "discard_compose",
        }
    }
}

/// 单轮维护结果，只包含安全统计，不含邮件正文。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MailboxMaintenanceReport {
    pub compose_clean: bool,
    pub compose_failure: Option<ComposeCleanupFailure>,
    pub matched_delivery: Option<Uuid>,
    pub discarded: bool,
    pub compose_matches: usize,
    pub sent_observed: usize,
    pub sent_recovered: usize,
    pub sent_candidates: usize,
    pub sent_matched: usize,
    /// Sent 列表、详情、通知预期或 Inbox 资产读取失败的次数；大于 0 即本轮部分失败。
    pub sent_state_failed: usize,
    /// 本轮退回收发队列重试的投递数。
    pub sweep_requeued: usize,
}

/// 维护服务；由 Worker 在固定周期调用，不存在自动重试队列。
pub struct MailboxMaintenanceService {
    maintenance: Arc<dyn MoogleMailboxMaintenance>,
    store: Arc<dyn MoogleMailboxEvidenceStore>,
    clock: Arc<dyn Clock>,
}

impl MailboxMaintenanceService {
    pub fn new(
        maintenance: Arc<dyn MoogleMailboxMaintenance>,
        store: Arc<dyn MoogleMailboxEvidenceStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { maintenance, store, clock }
    }
}
