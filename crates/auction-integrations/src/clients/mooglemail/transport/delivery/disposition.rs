//! 寄售处置的 Take / Return 执行：只在持有一次性 `PostPermit` 时发出 POST。
//!
//! 写前使用同一认证会话重读整封冻结清单；身份、世界、正文、Credits/COD 或逐项附件任一变化
//! 都零 POST。POST 后只有完整且逐项可配对的证明才返回已证明；HTTP 结果不清时保留未知待证。

use auction_application::consignment::PostPermit;
use auction_domain::consignment::{
    ExpectedAttachment, OriginalMailKey, ProofValidatorStatus, RemovedMarker, ValidatedMailProof,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::clients::mooglemail::gateway::PreparedConsignmentDisposition;
use crate::clients::mooglemail::parser::{
    ConsignmentDetailObservation, FrozenMailAttachment, FrozenMailManifest, observe_take_receipt,
};
use crate::clients::mooglemail::transport::types::MoogleMailDeliveryTransport;
use crate::clients::mooglemail::{MoogleMailErrorCategory, MoogleMailForm};

/// 寄售处置验证器版本；真实成对样本签收前固定为未签收。
pub const CONSIGNMENT_DISPOSITION_VALIDATOR_VERSION: &str =
    "mooglemail-consignment-disposition-v1-unsigned";

/// 寄售资产动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DispositionAction {
    /// 领取原信附件。
    Take,
    /// 原样退回。
    Return,
}

impl DispositionAction {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Take => "take",
            Self::Return => "return",
        }
    }
}

/// 冻结的处置目标；写前清单与决定/工作/attempt/fence 在此绑定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispositionTarget {
    /// 最终决定 UUID。
    pub final_decision_id: Uuid,
    /// 原工作 UUID。
    pub work_id: Uuid,
    /// 投递 UUID。
    pub delivery_id: Uuid,
    /// 本次 attempt UUID。
    pub attempt_id: Uuid,
    /// 起写 fence。
    pub fence: u64,
    /// 写前冻结清单。
    pub frozen: FrozenMailManifest,
}

impl DispositionTarget {
    /// 只接受非 nil 身份、正 fence 和与服务端观察一致的清单。
    pub fn new(
        final_decision_id: Uuid,
        work_id: Uuid,
        delivery_id: Uuid,
        attempt_id: Uuid,
        fence: u64,
        frozen: FrozenMailManifest,
    ) -> Option<Self> {
        (!final_decision_id.is_nil()
            && !work_id.is_nil()
            && !delivery_id.is_nil()
            && !attempt_id.is_nil()
            && fence > 0
            && !frozen.external_mail_id.is_empty())
        .then_some(Self { final_decision_id, work_id, delivery_id, attempt_id, fence, frozen })
    }
}

/// Return 必须携带的写前清单；类型上不可省略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedReturn(FrozenMailManifest);

impl ExpectedReturn {
    /// 用同一冻结清单构造；调用方不能只给邮件 ID。
    pub fn new(frozen: FrozenMailManifest) -> Self {
        Self(frozen)
    }

    /// 借用清单。
    pub fn frozen(&self) -> &FrozenMailManifest {
        &self.0
    }
}

/// 已授权的处置请求；Return 分支必须携带 `ExpectedReturn`。
///
/// ```compile_fail,E0063
/// fn missing(target: auction_integrations::mooglemail::DispositionTarget) {
///     let _ = auction_integrations::mooglemail::DispositionRequest::Return {
///         target: Box::new(target),
///     };
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispositionRequest {
    /// 领取。
    Take(DispositionTarget),
    /// 原样退回。
    Return {
        /// 目标清单。
        target: Box<DispositionTarget>,
        /// 必填的写前退回证明。
        expected_return: ExpectedReturn,
    },
}

impl DispositionRequest {
    /// 目标。
    pub fn target(&self) -> &DispositionTarget {
        match self {
            Self::Take(target) => target,
            Self::Return { target, .. } => target,
        }
    }

    /// 动作。
    pub const fn action(&self) -> DispositionAction {
        match self {
            Self::Take(_) => DispositionAction::Take,
            Self::Return { .. } => DispositionAction::Return,
        }
    }

    /// Return 的写前证明。
    pub fn expected_return(&self) -> Option<&ExpectedReturn> {
        match self {
            Self::Take(_) => None,
            Self::Return { expected_return, .. } => Some(expected_return),
        }
    }

    /// 构造后自检；Return 的 `expected_return` 必须与目标清单逐项相等。
    pub fn validate(&self) -> bool {
        let target = self.target();
        !target.final_decision_id.is_nil()
            && !target.work_id.is_nil()
            && !target.delivery_id.is_nil()
            && !target.attempt_id.is_nil()
            && target.fence > 0
            && !target.frozen.sender.is_empty()
            && !target.frozen.recipient.is_empty()
            && !target.frozen.subject.is_empty()
            && target.frozen.credits <= i64::MAX as u64
            && target.frozen.cod <= i64::MAX as u64
            && self.expected_return().is_none_or(|expected| expected.frozen() == &target.frozen)
    }
}

/// 写前失败原因；全部保证零 POST。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotSentReason {
    /// 冻结清单与当前页面不一致。
    ManifestChanged,
    /// 认证身份、世界或凭据代际变化。
    IdentityChanged,
    /// 当前页面不再允许该动作。
    ActionUnavailable,
    /// 准备预算已过。
    DeadlineExpired,
    /// 许可与请求的目标不一致。
    PermitMismatch,
    /// POST 尚未进入远端。
    WriteNotStarted,
    /// 会话或请求形状不可用。
    RequestInvalid,
    /// 临时限流、战斗或网络失败；仍未发送，稍后可重试。
    TemporaryUnavailable,
}

impl NotSentReason {
    /// 稳定 wire 与审计码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ManifestChanged => "manifest_changed",
            Self::IdentityChanged => "identity_changed",
            Self::ActionUnavailable => "action_unavailable",
            Self::DeadlineExpired => "deadline_expired",
            Self::PermitMismatch => "permit_mismatch",
            Self::WriteNotStarted => "write_not_started",
            Self::RequestInvalid => "request_invalid",
            Self::TemporaryUnavailable => "temporary_unavailable",
        }
    }
}

/// 已发出但结果待证的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SentUnprovenReason {
    /// POST 已发出但远端结果未知。
    WriteUnknown,
    /// POST 成功返回但证明不完整或冲突。
    ProofUnproven,
}

impl SentUnprovenReason {
    /// 稳定 wire 与审计码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WriteUnknown => "write_unknown",
            Self::ProofUnproven => "proof_unproven",
        }
    }
}

/// 已证明的处置结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispositionProof {
    action: DispositionAction,
    validator_status: ProofValidatorStatus,
    validator_version: &'static str,
    proof_digest: [u8; 32],
    after_page_digest: Option<[u8; 32]>,
    take: Option<ValidatedMailProof>,
}

impl DispositionProof {
    /// 动作。
    pub const fn action(&self) -> DispositionAction {
        self.action
    }

    /// 验证器签收状态；调用方必须显式读取，不能假定 `Proven` 就可授权。
    pub const fn validator_status(&self) -> ProofValidatorStatus {
        self.validator_status
    }

    /// 验证器版本。
    pub const fn validator_version(&self) -> &'static str {
        self.validator_version
    }

    /// 不含秘密的证明摘要。
    pub const fn proof_digest(&self) -> [u8; 32] {
        self.proof_digest
    }

    /// 写后页面独立重算的清单摘要；Return 没有单独的清单摘要。
    pub const fn after_page_digest(&self) -> Option<[u8; 32]> {
        self.after_page_digest
    }

    /// 只读的 Take 观察，仅供核对、隔离和追加证据；不得据此授予处分权。
    pub fn take_observation(&self) -> Option<&ValidatedMailProof> {
        self.take.as_ref()
    }

    /// 只有已签收的证明才能转成可授权形式；4D1 的未签收证明固定返回 `None`。
    pub fn into_signed(self) -> Option<SignedProof> {
        let take_signed = self
            .take
            .as_ref()
            .is_none_or(|take| take.validator_status == ProofValidatorStatus::SignedOff);
        (self.validator_status == ProofValidatorStatus::SignedOff && take_signed)
            .then_some(SignedProof(self))
    }
}

/// 已签收、可交给领域处分证明写端的包装；只能由 [`DispositionProof::into_signed`] 构造。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedProof(DispositionProof);

impl SignedProof {
    /// 动作。
    pub const fn action(&self) -> DispositionAction {
        self.0.action()
    }

    /// Take 的领域证明；Return 为空。
    pub fn take(&self) -> Option<&ValidatedMailProof> {
        self.0.take.as_ref()
    }

    /// 取出 Take 的领域证明，供领域写端按值消费。
    pub fn into_take(self) -> Option<ValidatedMailProof> {
        self.0.take
    }
}

/// 执行结果闭集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispositionOutcome {
    /// 确定未发出。
    NotSent {
        /// 安全原因。
        reason: NotSentReason,
    },
    /// 已发出、结果待证；不得自动重发。
    SentUnproven {
        /// 安全原因。
        reason: SentUnprovenReason,
        /// 远端错误分类；证明不完整时为空。
        error_category: Option<MoogleMailErrorCategory>,
    },
    /// 已证明。
    Proven(Box<DispositionProof>),
}

/// 消费一次性许可并执行一次处置；`PostPermit` 按值移动，不能重放。
///
/// 调用方必须持有由同一工作、投递与 fence 签发的许可；函数本身不缓存任何许可。
///
/// 没有许可无法调用 POST 路径：
///
/// ```compile_fail,E0061
/// async fn without_permit(
///     transport: auction_integrations::mooglemail::MoogleMailDeliveryTransport,
///     prepared: auction_integrations::mooglemail::PreparedConsignmentDisposition,
/// ) {
///     let _ = transport.execute_consignment_disposition(prepared).await;
/// }
/// ```
///
/// ```compile_fail,E0382
/// fn consume(_: auction_application::consignment::PostPermit) {}
/// fn example(permit: auction_application::consignment::PostPermit) {
///     consume(permit);
///     consume(permit);
/// }
/// ```
pub async fn execute_prepared_consignment_disposition(
    prepared: PreparedConsignmentDisposition,
    permit: PostPermit,
) -> DispositionOutcome {
    let request = prepared.request();
    if !request.validate() {
        return not_sent(NotSentReason::RequestInvalid);
    }
    let target = request.target();
    if permit.work_id() != target.work_id
        || permit.delivery_id() != target.delivery_id
        || permit.fence() != target.fence
    {
        return not_sent(NotSentReason::PermitMismatch);
    }
    let revalidated = match prepared.revalidate().await {
        Ok(revalidated) => revalidated,
        Err(reason) => return not_sent(reason),
    };
    let form = match request.action() {
        DispositionAction::Take => MoogleMailForm::take(revalidated.action_state().token()),
        DispositionAction::Return => {
            MoogleMailForm::return_message(revalidated.action_state().token())
        }
    };
    let form = match form {
        Ok(form) => form,
        Err(error) => return not_sent(form_error(error)),
    };
    let response =
        match prepared.client().post(revalidated.target(), prepared.session().cookie(), form).await
        {
            Ok(response) => response,
            Err(error) if !error.write_started() => {
                return not_sent(NotSentReason::WriteNotStarted);
            }
            Err(error) => {
                return DispositionOutcome::SentUnproven {
                    reason: SentUnprovenReason::WriteUnknown,
                    error_category: Some(error.category),
                };
            }
        };
    match request.action() {
        DispositionAction::Take => {
            let receipt =
                observe_take_receipt(revalidated.observation(), &response, target.frozen.world);
            let paired = pair_markers(&target.frozen.attachments, &receipt.markers);
            if !(receipt.identity_body_matches
                && receipt.assets_empty
                && receipt.assets_unavailable
                && receipt.credits == 0
                && receipt.cod == 0
                && receipt.receipt_direction_matches
                && paired)
            {
                return DispositionOutcome::SentUnproven {
                    reason: SentUnprovenReason::ProofUnproven,
                    error_category: None,
                };
            }
            let Ok(mail_key) = OriginalMailKey::new(
                &revalidated.binding().authenticated_uid,
                revalidated.binding().world,
                &target.frozen.external_mail_id,
            ) else {
                return DispositionOutcome::SentUnproven {
                    reason: SentUnprovenReason::ProofUnproven,
                    error_category: None,
                };
            };
            let proof = ValidatedMailProof {
                validator_status: ProofValidatorStatus::Unsigned,
                mail_key,
                decision_id: target.final_decision_id,
                work_id: target.work_id,
                delivery_id: target.delivery_id,
                attempt_id: target.attempt_id,
                fence: target.fence,
                session_binding_digest: revalidated.binding().digest,
                expected_manifest_digest: target.frozen.manifest_digest,
                // 写前重读已与冻结清单逐项比较；这里从重读解析结果独立重算，
                // 不复制冻结值，领域校验仍核 observed == expected。
                observed_manifest_digest: revalidated.observation().manifest_digest(),
                identity_body_matches: receipt.identity_body_matches,
                assets_empty: receipt.assets_empty,
                assets_unavailable: receipt.assets_unavailable,
                credits: receipt.credits,
                cod: receipt.cod,
                receipt_direction_matches: receipt.receipt_direction_matches,
                expected_attachments: expected_attachments(&target.frozen.attachments),
                removed_markers: receipt.markers.clone(),
            };
            DispositionOutcome::Proven(Box::new(DispositionProof {
                action: DispositionAction::Take,
                validator_status: ProofValidatorStatus::Unsigned,
                validator_version: CONSIGNMENT_DISPOSITION_VALIDATOR_VERSION,
                proof_digest: proof_digest(
                    DispositionAction::Take,
                    &target.frozen,
                    &receipt.markers,
                    Some(receipt.after_page_digest),
                ),
                after_page_digest: Some(receipt.after_page_digest),
                take: Some(proof),
            }))
        }
        DispositionAction::Return => {
            if prepared
                .parser()
                .prove_return_completed(
                    revalidated.action_state(),
                    &response,
                    target.frozen.world,
                    &target.frozen.external_mail_id,
                )
                .is_err()
            {
                return DispositionOutcome::SentUnproven {
                    reason: SentUnprovenReason::ProofUnproven,
                    error_category: None,
                };
            }
            let mut digest = Sha256::new();
            digest.update(b"auction/consignment/disposition-proof/v1\0return");
            digest.update(CONSIGNMENT_DISPOSITION_VALIDATOR_VERSION.as_bytes());
            digest.update(target.frozen.manifest_digest);
            digest.update(Sha256::digest(&response));
            DispositionOutcome::Proven(Box::new(DispositionProof {
                action: DispositionAction::Return,
                validator_status: ProofValidatorStatus::Unsigned,
                validator_version: CONSIGNMENT_DISPOSITION_VALIDATOR_VERSION,
                proof_digest: digest.finalize().into(),
                after_page_digest: None,
                take: None,
            }))
        }
    }
}

impl MoogleMailDeliveryTransport {
    /// 兼容既有发送器入口；执行逻辑与 [`execute_prepared_consignment_disposition`] 相同。
    pub async fn execute_consignment_disposition(
        &self,
        prepared: PreparedConsignmentDisposition,
        permit: PostPermit,
    ) -> DispositionOutcome {
        execute_prepared_consignment_disposition(prepared, permit).await
    }
}

fn not_sent(reason: NotSentReason) -> DispositionOutcome {
    DispositionOutcome::NotSent { reason }
}

fn form_error(_: crate::clients::mooglemail::MoogleMailError) -> NotSentReason {
    NotSentReason::RequestInvalid
}

fn expected_attachments(attachments: &[FrozenMailAttachment]) -> Vec<ExpectedAttachment> {
    attachments
        .iter()
        .map(|attachment| ExpectedAttachment {
            ordinal: attachment.ordinal,
            name: attachment.name.clone(),
            canonical_eid: attachment.canonical_eid.clone(),
        })
        .collect()
}

fn pair_markers(expected: &[FrozenMailAttachment], markers: &[RemovedMarker]) -> bool {
    if expected.is_empty() || markers.len() != expected.len() {
        return false;
    }
    let mut used = vec![false; expected.len()];
    for marker in markers {
        let mut candidate = None;
        for (index, attachment) in expected.iter().enumerate() {
            if used[index] || attachment.name != marker.name {
                continue;
            }
            if let Some(marker_eid) = marker.canonical_eid.as_deref()
                && attachment.canonical_eid.as_deref() != Some(marker_eid)
            {
                continue;
            }
            if candidate.replace(index).is_some() {
                return false;
            }
        }
        let Some(index) = candidate else { return false };
        used[index] = true;
    }
    used.iter().all(|used| *used)
}

fn proof_digest(
    action: DispositionAction,
    frozen: &FrozenMailManifest,
    markers: &[RemovedMarker],
    after_page_digest: Option<[u8; 32]>,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"auction/consignment/disposition-proof/v1\0");
    digest.update(action.as_str().as_bytes());
    digest.update(CONSIGNMENT_DISPOSITION_VALIDATOR_VERSION.as_bytes());
    digest.update(frozen.manifest_digest);
    if let Some(after_page_digest) = after_page_digest {
        digest.update(b"after-page\0");
        digest.update(after_page_digest);
    }
    digest.update(u64::try_from(markers.len()).unwrap_or(u64::MAX).to_be_bytes());
    for marker in markers {
        digest.update(u64::try_from(marker.name.len()).unwrap_or(u64::MAX).to_be_bytes());
        digest.update(marker.name.as_bytes());
        match marker.canonical_eid.as_deref() {
            Some(eid) => {
                digest.update([1]);
                digest.update(eid.as_bytes());
            }
            None => digest.update([0]),
        }
    }
    digest.finalize().into()
}

/// 仅供同 crate 的预检失败映射使用。
pub(crate) fn manifest_matches(
    frozen: &FrozenMailManifest,
    observation: &ConsignmentDetailObservation,
) -> bool {
    observation.frozen_manifest().as_ref() == Some(frozen)
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::deposit::RemovedMarker;

    use super::pair_markers;
    use crate::mooglemail::FrozenMailAttachment;

    fn attachment(ordinal: u32, name: &str, eid: &str) -> FrozenMailAttachment {
        FrozenMailAttachment { ordinal, name: name.to_owned(), canonical_eid: Some(eid.to_owned()) }
    }

    fn marker(name: &str, eid: &str) -> RemovedMarker {
        RemovedMarker { name: name.to_owned(), canonical_eid: Some(eid.to_owned()) }
    }

    #[test]
    fn markers_pair_by_real_display_name_and_eid_in_any_order() {
        let expected = vec![attachment(1, "Fake Sword", "42"), attachment(2, "Fake Shield", "43")];
        assert!(pair_markers(
            &expected,
            &[marker("Fake Shield", "43"), marker("Fake Sword", "42")]
        ));
        let placeholders = [marker("attachment-2", "43"), marker("attachment-1", "42")];
        assert!(!pair_markers(&expected, &placeholders));
    }
}
