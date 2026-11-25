//! 派发五个操作与 Worker 收款认证（`certify_cod`）的规范编码、摘要与系统命令 ID（v2 统一前缀）。
//!
//! 流为 `prefix || text(operation) || text(actor 类型) || text(服务引用) || target(UUID 原 16 字节) ||
//! inputs`；非负整数 u64 大端，文本是 u64 字节长度加 UTF-8，时间为 i64 秒加 u32 纳秒，均大端。
//! `request_id` 不入摘要，`command_id` 是账本键也不入摘要。命令 ID 为
//! `UUIDv5(NAMESPACE_URL, prefix || text(operation) || 业务键)`，业务键见各 `*_business_key`。

use auction_domain::CommandId;
use auction_domain::consignment::dispatch::{
    AttemptKey, DispatchRoundKey, EvidenceCategory, FrozenDispatchIdentity, RoundOrigin,
    dispatch_attempt_id, dispatch_round_id,
};
use auction_domain::consignment::funding::FundingMode;
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::{
    CertifyCodFacts, CollectedEvidence, DepositActor, DispatchOperation,
};
use crate::ports::core::RequestHash;
use crate::ports::delivery::DeliverySendCredentialBinding;
use crate::ports::settlement::CodLeg;
use crate::services::consignment::deposit::encoding::COMMAND_PREFIX_V2;

/// 规范字节构建器。
struct Encoding {
    bytes: Vec<u8>,
}

impl Encoding {
    /// 写入前缀、操作、固定的 Worker 服务身份与目标 UUID。
    fn new(operation: DispatchOperation, target: Uuid) -> Self {
        Self::named(operation.as_str(), target)
    }

    /// 同 [`Encoding::new`]，操作名不在派发闭集里（如资金 owner 的 `certify_cod`）。
    fn named(operation: &str, target: Uuid) -> Self {
        let mut encoding = Self { bytes: COMMAND_PREFIX_V2.to_vec() };
        encoding.text(operation);
        encoding.text(DepositActor::Worker.kind());
        encoding.text(DepositActor::DEPOSIT_SERVICE_REF);
        encoding.uuid(target);
        encoding
    }

    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn uuid(&mut self, value: Uuid) {
        self.bytes.extend_from_slice(value.as_bytes());
    }

    fn text(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes.extend_from_slice(value.as_bytes());
    }

    fn digest(&mut self, value: &[u8; 32]) {
        self.bytes.extend_from_slice(value);
    }

    fn timestamp(&mut self, value: Timestamp) {
        self.bytes.extend_from_slice(&value.timestamp().to_be_bytes());
        self.bytes.extend_from_slice(&value.timestamp_subsec_nanos().to_be_bytes());
    }

    fn request_hash(&self) -> RequestHash {
        RequestHash::from_canonical_bytes(&self.bytes)
    }
}

/// 系统命令身份：`UUIDv5(NAMESPACE_URL, prefix || text(operation) || 业务键)`。
fn command_id(operation: DispatchOperation, business_key: &[u8]) -> CommandId {
    named_command_id(operation.as_str(), business_key)
}

/// 同 [`command_id`]，操作名不在派发闭集里。
fn named_command_id(operation: &str, business_key: &[u8]) -> CommandId {
    let mut input = COMMAND_PREFIX_V2.to_vec();
    input.extend_from_slice(&(operation.len() as u64).to_be_bytes());
    input.extend_from_slice(operation.as_bytes());
    input.extend_from_slice(business_key);
    CommandId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, &input))
}

fn round_key_bytes(key: &DispatchRoundKey) -> Vec<u8> {
    let mut bytes = key.contract_id.as_bytes().to_vec();
    bytes.extend_from_slice(key.obligation_id.as_bytes());
    bytes.extend_from_slice(&u64::from(key.round).to_be_bytes());
    bytes
}

fn attempt_key_bytes(key: &AttemptKey) -> Vec<u8> {
    let mut bytes = key.delivery_id.as_bytes().to_vec();
    bytes.extend_from_slice(&key.fence.to_be_bytes());
    bytes
}

/// `authorize_dispatch`：业务键为义务加轮；摘要含来源、投递与完整冻结身份摘要。
pub(super) fn authorize(
    key: &DispatchRoundKey,
    origin: &RoundOrigin,
    delivery_id: Uuid,
    frozen: &FrozenDispatchIdentity,
) -> (CommandId, RequestHash) {
    let round_id = dispatch_round_id(key);
    let mut encoding = Encoding::new(DispatchOperation::AuthorizeDispatch, round_id);
    encoding.uuid(key.contract_id);
    encoding.uuid(key.obligation_id);
    encoding.u64(u64::from(key.round));
    match origin {
        RoundOrigin::First => encoding.bytes.push(0),
        RoundOrigin::Redispatch { closed_by_return_intake } => {
            encoding.bytes.push(1);
            encoding.uuid(*closed_by_return_intake);
        }
    }
    encoding.uuid(delivery_id);
    encoding.digest(&frozen.identity_digest());
    (
        command_id(DispatchOperation::AuthorizeDispatch, &round_key_bytes(key)),
        encoding.request_hash(),
    )
}

/// `start_attempt`：业务键为投递加 fence；摘要含轮、claim token 与冻结身份摘要。
pub(super) fn start_attempt(
    round_id: Uuid,
    attempt: &AttemptKey,
    claim_token: Uuid,
    frozen: &FrozenDispatchIdentity,
) -> (CommandId, RequestHash) {
    let mut encoding = Encoding::new(DispatchOperation::StartAttempt, round_id);
    encoding.uuid(attempt.delivery_id);
    encoding.u64(attempt.fence);
    encoding.uuid(claim_token);
    encoding.digest(&frozen.identity_digest());
    (
        command_id(DispatchOperation::StartAttempt, &attempt_key_bytes(attempt)),
        encoding.request_hash(),
    )
}

/// `mark_write_started`：业务键为尝试 ID；摘要含投递、fence、claim token 与凭据绑定。
pub(super) fn mark_write_started(
    attempt: &AttemptKey,
    claim_token: Uuid,
    credential: Option<&DeliverySendCredentialBinding>,
) -> (CommandId, RequestHash) {
    let attempt_id = dispatch_attempt_id(attempt);
    let mut encoding = Encoding::new(DispatchOperation::MarkWriteStarted, attempt_id);
    encoding.uuid(attempt.delivery_id);
    encoding.u64(attempt.fence);
    encoding.uuid(claim_token);
    match credential {
        None => encoding.bytes.push(0),
        Some(binding) => {
            encoding.bytes.push(1);
            encoding.uuid(binding.credential_id);
            encoding.i64(binding.credential_version);
            encoding.i64(binding.runtime_generation);
        }
    }
    (
        command_id(DispatchOperation::MarkWriteStarted, attempt_id.as_bytes()),
        encoding.request_hash(),
    )
}

/// `record_evidence` 的输入；证据身份是尝试加类别加远端观察，其余事实全部进摘要。
pub(super) struct EvidenceFacts<'a> {
    pub(super) attempt: &'a AttemptKey,
    pub(super) category: EvidenceCategory,
    pub(super) remote_observation_id: &'a str,
    pub(super) observed_at: Timestamp,
    pub(super) validator_version: &'a str,
    pub(super) evidence_digest: &'a [u8; 32],
}

/// `record_evidence`：业务键为尝试 ID、类别与远端观察身份。
pub(super) fn record_evidence(facts: &EvidenceFacts<'_>) -> (CommandId, RequestHash) {
    let attempt_id = dispatch_attempt_id(facts.attempt);
    let mut encoding = Encoding::new(DispatchOperation::RecordEvidence, attempt_id);
    encoding.text(facts.category.as_str());
    encoding.text(facts.remote_observation_id);
    encoding.timestamp(facts.observed_at);
    encoding.text(facts.validator_version);
    encoding.digest(facts.evidence_digest);
    let mut key = attempt_id.as_bytes().to_vec();
    key.extend_from_slice(&(facts.category.as_str().len() as u64).to_be_bytes());
    key.extend_from_slice(facts.category.as_str().as_bytes());
    key.extend_from_slice(&(facts.remote_observation_id.len() as u64).to_be_bytes());
    key.extend_from_slice(facts.remote_observation_id.as_bytes());
    (command_id(DispatchOperation::RecordEvidence, &key), encoding.request_hash())
}

/// `prove_handoff`：业务键为原物义务加原轮（与 `authorize_dispatch` 同一轮键字节）；目标为该义务，
/// 摘要含尝试与所消费确领证据的远端观察身份及摘要。
pub(super) fn prove_handoff(
    key: &DispatchRoundKey,
    attempt: &AttemptKey,
    remote_observation_id: &str,
    evidence_digest: &[u8; 32],
) -> (CommandId, RequestHash) {
    let mut encoding = Encoding::new(DispatchOperation::ProveHandoff, key.obligation_id);
    encoding.uuid(key.contract_id);
    encoding.uuid(key.obligation_id);
    encoding.u64(u64::from(key.round));
    encoding.uuid(attempt.delivery_id);
    encoding.u64(attempt.fence);
    encoding.text(remote_observation_id);
    encoding.digest(evidence_digest);
    (command_id(DispatchOperation::ProveHandoff, &round_key_bytes(key)), encoding.request_hash())
}

/// `certify_cod` 的账本操作名（与资金 owner 的回执账本一致）。
const CERTIFY_COD: &str = "certify_cod";

/// `certify_cod`：业务键为原付款义务加原投递（D9 §6.2）；目标为原付款义务。摘要含合同、实收业务键、世界、
/// 模式、P / S / F、毛额、冻结派发事实与所消费付款证据的远端观察身份及摘要。opening 份额恒为“未登记”（本入口
/// 不传份额）。严格现金的新鲜现金观察是资金锁内复验的前置条件，每次重读都会变化，不进摘要：否则提交结果未知
/// 后的同 ID 重试会被当成异请求；它不构成被消费的经济事实（不变量 1）。
pub(super) fn certify_cod(
    facts: &CertifyCodFacts,
    evidence: &CollectedEvidence,
) -> (CommandId, RequestHash) {
    let mut encoding = Encoding::named(CERTIFY_COD, facts.payment_obligation_id);
    encoding.uuid(facts.contract_id);
    encoding.uuid(facts.payment_obligation_id);
    encoding.uuid(facts.original_delivery_id);
    encoding.text(&facts.receipt.uid);
    encoding.text(&facts.receipt.payment_mail_id);
    encoding.text(match facts.receipt.leg {
        CodLeg::Original => "original",
        CodLeg::Carrier => "carrier",
    });
    encoding.text(facts.world.as_str());
    encoding.text(match facts.mode {
        FundingMode::StrictCash => "strict_cash",
        FundingMode::RecordOnly => "record_only",
    });
    encoding.i64(facts.price.value());
    encoding.i64(facts.quote.fee);
    encoding.i64(facts.quote.seller_amount);
    encoding.i64(facts.gross);
    encoding.digest(&facts.freeze.prepare_digest);
    encoding.digest(&facts.freeze.activation_digest);
    encoding.digest(&facts.freeze.body_digest);
    encoding.u64(facts.freeze.equipment_ids.len() as u64);
    for equipment_id in &facts.freeze.equipment_ids {
        encoding.text(equipment_id);
    }
    encoding.text(&evidence.remote_observation_id);
    encoding.digest(&evidence.evidence_digest);
    let mut key = facts.payment_obligation_id.as_bytes().to_vec();
    key.extend_from_slice(facts.original_delivery_id.as_bytes());
    (named_command_id(CERTIFY_COD, &key), encoding.request_hash())
}
