//! 退件接管三个操作的规范编码、摘要与系统命令 ID（v2 统一前缀）。
//!
//! 流为 `prefix || text(operation) || text(actor 类型) || text(服务引用) || target(UUID 原 16 字节) ||
//! inputs`；非负整数 u64 大端，文本是 u64 字节长度加 UTF-8。`request_id` 不入摘要，`command_id` 是
//! 账本键也不入摘要；RTS 观察 ID 每次到达都可能不同，不进接管摘要（同一 K 重复到达得原回执）。
//! 起写命令 ID 沿领域 `start_return_take_command_id`；接管与消费的 ID 为
//! `UUIDv5(NAMESPACE_URL, prefix || text(operation) || 业务键)`。

use auction_domain::CommandId;
use auction_domain::consignment::returns::{
    ReturnIntakeFacts, return_intake_id, start_return_take_command_id,
};
use auction_domain::consignment::settlement_world_code;
use uuid::Uuid;

use super::commands::{ConsumeReturnInput, StartTakeInput};
use crate::ports::consignment::{DepositActor, ReturnOperation};
use crate::ports::core::RequestHash;
use crate::services::consignment::deposit::encoding::COMMAND_PREFIX_V2;

/// 规范字节构建器。
struct Encoding {
    bytes: Vec<u8>,
}

impl Encoding {
    /// 写入前缀、操作、actor 类型、固定服务引用与目标 UUID。
    fn new(operation: ReturnOperation, actor: &DepositActor, target: Uuid) -> Self {
        let mut encoding = Self { bytes: COMMAND_PREFIX_V2.to_vec() };
        encoding.text(operation.as_str());
        encoding.text(actor.kind());
        encoding.text(DepositActor::DEPOSIT_SERVICE_REF);
        encoding.uuid(target);
        encoding
    }

    fn u64(&mut self, value: u64) {
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

    fn request_hash(&self) -> RequestHash {
        RequestHash::from_canonical_bytes(&self.bytes)
    }
}

/// 系统命令身份：`UUIDv5(NAMESPACE_URL, prefix || text(operation) || 业务键)`。
fn command_id(operation: ReturnOperation, business_key: &[u8]) -> CommandId {
    let mut input = COMMAND_PREFIX_V2.to_vec();
    input.extend_from_slice(&(operation.as_str().len() as u64).to_be_bytes());
    input.extend_from_slice(operation.as_str().as_bytes());
    input.extend_from_slice(business_key);
    CommandId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, &input))
}

/// `acquire_return`：业务键为 RTS 的 K；摘要含原合同、义务、原轮、应退腿，以及接管的决定参数
/// （Sent MID、原 attempt 与 fence、冻结清单摘要、授权的原会话）：同 K 异决定参数是异请求，
/// 账本得 `command_id_conflict` 而不是被当作重放（不变量 1，D9 §6.2）。
pub(super) fn acquire(facts: &ReturnIntakeFacts) -> (CommandId, RequestHash) {
    let mut encoding = Encoding::new(
        ReturnOperation::AcquireReturn,
        &DepositActor::System,
        return_intake_id(&facts.k),
    );
    encoding.uuid(facts.contract_id);
    encoding.uuid(facts.obligation_id);
    encoding.u64(u64::from(facts.original_round));
    encoding.text(facts.leg.as_str());
    encoding.text(&facts.sent_mid);
    encoding.uuid(facts.original_attempt.delivery_id);
    encoding.u64(facts.original_attempt.fence);
    encoding.digest(&facts.frozen_manifest_digest);
    encoding.uuid(facts.session_id);
    let mut key = Vec::new();
    for part in [
        facts.k.authenticated_bot_uid(),
        settlement_world_code(facts.k.world()),
        facts.k.external_mid(),
    ] {
        key.extend_from_slice(&(part.len() as u64).to_be_bytes());
        key.extend_from_slice(part.as_bytes());
    }
    (command_id(ReturnOperation::AcquireReturn, &key), encoding.request_hash())
}

/// `start_return_take`：命令 ID 沿领域派生；摘要含工作、投递、attempt、fence、claim token 与两个摘要。
pub(super) fn start_take(input: &StartTakeInput) -> (CommandId, RequestHash) {
    let mut encoding =
        Encoding::new(ReturnOperation::StartReturnTake, &DepositActor::Worker, input.intake_id);
    encoding.uuid(input.work_id);
    encoding.uuid(input.delivery_id);
    encoding.uuid(input.attempt_id);
    encoding.u64(input.fence);
    encoding.uuid(input.claim_token);
    encoding.digest(&input.session_binding_digest);
    encoding.digest(&input.expected_manifest_digest);
    (
        start_return_take_command_id(input.intake_id, input.work_id, input.attempt_id, input.fence),
        encoding.request_hash(),
    )
}

/// `consume_return`：业务键为 intake（每个接管只消费一次）；摘要含原起写引用与证明身份。
pub(super) fn consume(input: &ConsumeReturnInput) -> (CommandId, RequestHash) {
    let mut encoding =
        Encoding::new(ReturnOperation::ConsumeReturn, &DepositActor::Worker, input.intake_id);
    encoding.uuid(input.original_attempt_id);
    encoding.u64(input.fence);
    encoding.uuid(input.observation_id);
    encoding.text(&input.validator_version);
    encoding.digest(&input.proof_digest);
    (
        command_id(ReturnOperation::ConsumeReturn, input.intake_id.as_bytes()),
        encoding.request_hash(),
    )
}
