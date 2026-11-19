//! 人工 Sent 证明的本地状态：一次性提交、可编辑草稿与形状问题码。
//!
//! 这些类型只承载浏览器侧投影。

use auction_web_shared::t;

use crate::api::{
    AdminApiFailure, DeliveryEvidence, DeliverySentEvidence, DeliverySentProofPreview,
    LocalTimeError, datetime_local_value,
};

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
    pub(super) sent_minute_edited: bool,
    pub(super) remaining_credits: String,
    pub(super) cod_pending: String,
    pub(super) collected: bool,
    pub(super) receipt: String,
    pub(super) reason: String,
}

impl SentProofDraft {
    pub(super) fn for_evidence(evidence: Option<&DeliveryEvidence>) -> Self {
        Self {
            evidence_reference: evidence.map_or_default(|item| item.reference.clone()),
            sender_name: String::new(),
            sender_uid: String::new(),
            // 本地值只供展示；时钟回拨时不能拿它反推原 UTC 分钟。
            sent_minute: evidence
                .and_then(|item| item.sent_at.as_deref())
                .and_then(datetime_local_value)
                .unwrap_or_default(),
            sent_minute_edited: false,
            remaining_credits: evidence
                .and_then(|item| item.remaining_credits)
                .map_or_default(|value| value.to_string()),
            cod_pending: evidence
                .and_then(|item| item.cod_pending)
                .map_or_default(|value| value.to_string()),
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
    SentMinute(LocalTimeError),
    SenderName,
    SenderUid,
    RemainingCredits,
    CodPending,
    Receipt,
    MissingObservedEquipmentIds,
}

impl SentProofIssue {
    pub(super) fn message(self) -> String {
        match self {
            Self::Evidence => t!("delivery-sent-proof-issue-evidence"),
            Self::Attempt => t!("delivery-sent-proof-issue-attempt"),
            Self::Digest => t!("delivery-sent-proof-issue-digest"),
            Self::Recipient => t!("delivery-sent-proof-issue-recipient"),
            Self::Subject => t!("delivery-sent-proof-issue-subject"),
            Self::SentMinute(LocalTimeError::Invalid) => {
                t!("delivery-sent-proof-issue-sent-minute")
            }
            Self::SentMinute(LocalTimeError::Nonexistent) => {
                t!("admin-auctions-nonexistent-local-time")
            }
            Self::SentMinute(LocalTimeError::Ambiguous) => {
                t!("admin-auctions-ambiguous-local-time")
            }
            Self::SenderName => t!("delivery-sent-proof-issue-sender-name"),
            Self::SenderUid => t!("delivery-sent-proof-issue-sender-uid"),
            Self::RemainingCredits | Self::CodPending => t!("delivery-sent-proof-issue-amount"),
            Self::Receipt => t!("delivery-sent-proof-issue-receipt"),
            Self::MissingObservedEquipmentIds => t!("delivery-sent-proof-missing-observed-ids"),
        }
    }

    pub(super) fn field_id(self) -> &'static str {
        match self {
            Self::Evidence | Self::MissingObservedEquipmentIds => "delivery-sent-evidence",
            Self::Attempt | Self::Digest | Self::Recipient | Self::Subject => {
                "delivery-sent-evidence-summary"
            }
            Self::SentMinute(_) => "delivery-sent-minute",
            Self::SenderName => "delivery-sent-sender-name",
            Self::SenderUid => "delivery-sent-sender-uid",
            Self::RemainingCredits => "delivery-sent-remaining-credits",
            Self::CodPending => "delivery-sent-cod-pending",
            Self::Receipt => "delivery-sent-receipt",
        }
    }
}

/// 发送到预览的请求和预览时的详情 fence。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SentProofRequest {
    pub(super) delivery_id: String,
    pub(super) fencing_version: i64,
    pub(super) evidence_reference: String,
    pub(super) evidence: DeliverySentEvidence,
}

#[derive(Debug, Clone, Default)]
pub(super) struct AcceptedResponsibilities {
    pub(super) collected_assets: bool,
    pub(super) prior_unbound_attempts: bool,
    pub(super) sent_before_current_attempt: bool,
}

#[derive(Debug, Clone)]
pub(super) struct ReviewedSentProof {
    request: SentProofRequest,
    pub(super) preview: DeliverySentProofPreview,
    pub(super) accepted: AcceptedResponsibilities,
}

impl ReviewedSentProof {
    pub(super) fn can_confirm(&self, delivery_id: &str, fencing_version: i64) -> bool {
        self.request.delivery_id == delivery_id
            && self.request.fencing_version == fencing_version
            && self.preview.eligible
            && (!self.preview.requires_collected_asset_acceptance || self.accepted.collected_assets)
            && (self.preview.prior_unbound_attempts == Some(false)
                || self.accepted.prior_unbound_attempts)
            && (self.preview.sent_before_current_attempt == Some(false)
                || self.accepted.sent_before_current_attempt)
    }

    /// 已发布命令用一个历史责任位；分别确认实际触发的所有理由后才可组装。
    pub(super) fn submission(&self, reason: &str) -> SentProofSubmission {
        SentProofSubmission {
            evidence_reference: self.request.evidence_reference.clone(),
            evidence: self.request.evidence.clone(),
            reason: reason.trim().to_owned(),
            expected_fencing_version: self.request.fencing_version,
            accept_collected_assets: self.preview.requires_collected_asset_acceptance
                && self.accepted.collected_assets,
            accept_attempt_history: self.preview.requires_attempt_history_acceptance,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum SentProofPreviewError {
    Local(SentProofIssue),
    Api(AdminApiFailure),
}

/// 点选、编辑或详情更新都使正在进行的预览和相关勾选失效。
#[derive(Debug, Clone, Default)]
pub(super) struct SentProofReview {
    revision: u64,
    pub(super) pending: bool,
    pub(super) outcome: Option<Result<ReviewedSentProof, SentProofPreviewError>>,
}

impl SentProofReview {
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn acceptance(&mut self, revision: u64) -> Option<&mut AcceptedResponsibilities> {
        if revision != self.revision {
            return None;
        }
        self.outcome.as_mut()?.as_mut().ok().map(|proof| &mut proof.accepted)
    }

    pub(super) fn clear(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.pending = false;
        self.outcome = None;
    }

    pub(super) fn begin(&mut self) -> u64 {
        self.clear();
        self.pending = true;
        self.revision
    }

    /// 返回当前预览是否要求重读详情。只有仍被等待的请求可以使投递详情失效。
    pub(super) fn complete(
        &mut self,
        revision: u64,
        request: SentProofRequest,
        result: Result<DeliverySentProofPreview, AdminApiFailure>,
    ) -> bool {
        // 迟到响应不能恢复旧责任，也不能清除另一份证据正在请求的加载状态。
        if revision != self.revision || !self.pending {
            return false;
        }
        self.pending = false;
        self.outcome = Some(result.map_err(SentProofPreviewError::Api).and_then(|preview| {
            if preview.expected_fencing_version != request.fencing_version
                || preview.delivery_id != request.delivery_id
                || preview.external_reference
                    != format!(
                        "mooglemail:{}:send:{}",
                        request.evidence.world_id, request.evidence.external_mail_id
                    )
            {
                return Err(SentProofPreviewError::Api(AdminApiFailure::client(
                    "stale_delivery_state",
                )));
            }
            Ok(ReviewedSentProof {
                request,
                preview,
                accepted: AcceptedResponsibilities::default(),
            })
        }));
        matches!(
            self.outcome.as_ref(),
            Some(Err(SentProofPreviewError::Api(error))) if error.code == "stale_delivery_state"
        )
    }
}
