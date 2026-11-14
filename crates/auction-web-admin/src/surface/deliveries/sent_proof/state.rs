//! 人工 Sent 证明的本地状态：一次性提交、可编辑草稿与形状问题码。
//!
//! 这些类型只承载浏览器侧投影；预填只复制证据中已有的字段，不伪造未观测值。

use auction_web_shared::t;

use crate::api::{DeliveryEvidence, DeliverySentEvidence};

use super::validation::sent_minute_from_timestamp;

/// 详情页转交的一次性提交；`command_id` 由命令宿主生成并在重试间保持。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SentProofSubmission {
    pub(crate) evidence_reference: String,
    pub(crate) evidence: DeliverySentEvidence,
    pub(crate) reason: String,
    pub(crate) expected_fencing_version: i64,
    pub(crate) accept_collected_assets: bool,
    pub(crate) accept_attempt_history: bool,
}

/// 操作员可编辑的观察草稿；预填只复制证据中已有的字段，不伪造未观测值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SentProofDraft {
    pub(super) evidence_reference: String,
    pub(super) sender_name: String,
    pub(super) sender_uid: String,
    pub(super) sent_minute: String,
    pub(super) remaining_credits: String,
    pub(super) cod_pending: String,
    pub(super) collected: bool,
    pub(super) receipt: String,
    pub(super) reason: String,
}

impl SentProofDraft {
    pub(super) fn for_evidence(evidence: Option<&DeliveryEvidence>) -> Self {
        Self {
            evidence_reference: evidence.map(|item| item.reference.clone()).unwrap_or_default(),
            sender_name: String::new(),
            sender_uid: String::new(),
            sent_minute: evidence
                .and_then(|item| item.sent_at.as_deref())
                .and_then(sent_minute_from_timestamp)
                .unwrap_or_default(),
            remaining_credits: evidence
                .and_then(|item| item.remaining_credits)
                .map(|value| value.to_string())
                .unwrap_or_default(),
            cod_pending: evidence
                .and_then(|item| item.cod_pending)
                .map(|value| value.to_string())
                .unwrap_or_default(),
            collected: evidence.and_then(|item| item.collected).unwrap_or(false),
            receipt: evidence.and_then(|item| item.receipt.clone()).unwrap_or_default(),
            reason: String::new(),
        }
    }

    /// 切换证据时保留操作员已填写的发送者与原因，只替换由证据派生的字段。
    pub(super) fn with_evidence(self, evidence: Option<&DeliveryEvidence>) -> Self {
        let mut next = Self::for_evidence(evidence);
        next.sender_name = self.sender_name;
        next.sender_uid = self.sender_uid;
        next.reason = self.reason;
        next
    }
}

/// 草稿形状问题；只在浏览器本地阻止提交，语义准入仍由服务端预览与事务裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SentProofIssue {
    Evidence,
    Attempt,
    Digest,
    Recipient,
    Subject,
    SentMinute,
    SenderName,
    SenderUid,
    Amount,
    Receipt,
}

impl SentProofIssue {
    pub(super) fn message(self) -> String {
        match self {
            Self::Evidence => t!("delivery-sent-proof-issue-evidence"),
            Self::Attempt => t!("delivery-sent-proof-issue-attempt"),
            Self::Digest => t!("delivery-sent-proof-issue-digest"),
            Self::Recipient => t!("delivery-sent-proof-issue-recipient"),
            Self::Subject => t!("delivery-sent-proof-issue-subject"),
            Self::SentMinute => t!("delivery-sent-proof-issue-sent-minute"),
            Self::SenderName => t!("delivery-sent-proof-issue-sender-name"),
            Self::SenderUid => t!("delivery-sent-proof-issue-sender-uid"),
            Self::Amount => t!("delivery-sent-proof-issue-amount"),
            Self::Receipt => t!("delivery-sent-proof-issue-receipt"),
        }
    }
}
