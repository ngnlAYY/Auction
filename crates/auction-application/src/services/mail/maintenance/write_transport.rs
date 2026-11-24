//! 4D1 处置适配器的应用层端口：准备、一次性许可执行与证明投影。

use std::any::Any;
use std::fmt;
use std::time::Instant;

use uuid::Uuid;

use super::frozen_manifest::{
    ConsignmentDispositionRequest, ConsignmentNotSent, ConsignmentNotSentReason,
    ConsignmentSentUnprovenReason,
};
use crate::ports::consignment::{
    ConsignmentError, ConsignmentFuture, DispositionAction, PostPermit,
};

/// 4D1 证明未暴露独立观察 UUID 时，以证明摘要与签收状态派生稳定观察身份。
///
/// 同一证明内容与同一签收状态重放得到同一 ID；不同证明内容或不同签收状态不能复用原观察
/// 身份。`signed` 必须原样取自 `DispositionProof::into_signed().is_some()`，不能用页面文本
/// 或适配器自己的判断代替。集成适配器必须在该关系成立后使用本函数或真实观察 ID。
pub fn disposition_proof_observation_id(proof_digest: [u8; 32], signed: bool) -> Uuid {
    let mut bytes = Vec::with_capacity(80);
    bytes.extend_from_slice(b"auction/consignment/disposition-proof-observation/v1\0");
    bytes.extend_from_slice(&proof_digest);
    match signed {
        true => bytes.extend_from_slice(b"\0signed"),
        false => bytes.extend_from_slice(b"\0unsigned"),
    }
    Uuid::new_v5(&Uuid::NAMESPACE_URL, &bytes)
}

/// 已准备但尚未起写的 4D1 会话；只暴露起写与证据绑定需要的安全摘要。
pub trait PreparedDisposition: Send {
    /// 认证会话绑定摘要。
    fn session_binding_digest(&self) -> [u8; 32];
    /// 写前同一会话重读到的清单摘要。
    fn observed_manifest_digest(&self) -> [u8; 32];
    /// 本次观察身份。
    fn observation_id(&self) -> Uuid;

    /// 适配器内部执行句柄；默认不提供，只有同一适配器的 `execute` 可以解释。
    fn into_execution_handle(self: Box<Self>) -> Option<Box<dyn Any + Send>> {
        None
    }
}

/// 4D1 处置适配器；实现方必须逐项完成严格预检与写后证明。
pub trait ConsignmentDispositionTransport: Send + Sync {
    /// 同一认证会话重读并准备；不得 POST。
    fn prepare(
        &self,
        request: ConsignmentDispositionRequest,
        deadline: Instant,
    ) -> ConsignmentFuture<'_, Result<Box<dyn PreparedDisposition>, ConsignmentNotSent>>;

    /// 消费一次性许可并执行一次 POST。
    fn execute(
        &self,
        prepared: Box<dyn PreparedDisposition>,
        permit: PostPermit,
        deadline: Instant,
    ) -> ConsignmentFuture<'_, ConsignmentDispositionExecution>;
}

/// 4D1 写后结果闭集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsignmentDispositionExecution {
    /// 确定未发出。
    NotSent {
        /// 原因。
        reason: ConsignmentNotSentReason,
        /// 本次重读摘要。
        observed_manifest_digest: Option<[u8; 32]>,
    },
    /// 已发出但结果待证；不得自动重发。
    SentUnproven {
        /// 原因。
        reason: ConsignmentSentUnprovenReason,
    },
    /// 已证明；只有 `into_signed()` 返回 `Some` 才可授权。
    Proven(ConsignmentDispositionProof),
}

/// 4D1 证明的应用层投影；`signed` 只能来自 `DispositionProof::into_signed()`。
#[derive(Clone, PartialEq, Eq)]
pub struct ConsignmentDispositionProof {
    action: DispositionAction,
    observation_id: Uuid,
    validator_version: String,
    proof_digest: [u8; 32],
    signed: bool,
}

impl ConsignmentDispositionProof {
    /// 由 4D1 适配器构造。`signed` 必须严格等于 `DispositionProof::into_signed().is_some()`，
    /// 适配器不得依据状态码、页面文本或自身判断另行填写；该等价关系由集成测试钉住。
    pub fn from_adapter(
        action: DispositionAction,
        observation_id: Uuid,
        validator_version: String,
        proof_digest: [u8; 32],
        signed: bool,
    ) -> Result<Self, ConsignmentError> {
        if observation_id.is_nil() || validator_version.is_empty() || proof_digest == [0; 32] {
            return Err(ConsignmentError::InvalidDepositRequest);
        }
        Ok(Self { action, observation_id, validator_version, proof_digest, signed })
    }

    /// 动作。
    pub const fn action(&self) -> DispositionAction {
        self.action
    }

    /// 证明观察身份。
    pub const fn observation_id(&self) -> Uuid {
        self.observation_id
    }

    /// 只有已签收证明可转成授权包装。
    pub fn into_signed(self) -> Option<SignedConsignmentProof> {
        self.signed.then_some(SignedConsignmentProof {
            action: self.action,
            observation_id: self.observation_id,
            validator_version: self.validator_version,
            proof_digest: self.proof_digest,
        })
    }
}

impl fmt::Debug for ConsignmentDispositionProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsignmentDispositionProof")
            .field("action", &self.action)
            .field("observation_id", &self.observation_id)
            .field("validator_version", &self.validator_version)
            .field("proof_digest", &self.proof_digest)
            .field("signed", &self.signed)
            .finish()
    }
}

/// 已签收、可交给证明写端的包装；只能由 `ConsignmentDispositionProof::into_signed` 构造。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedConsignmentProof {
    action: DispositionAction,
    observation_id: Uuid,
    validator_version: String,
    proof_digest: [u8; 32],
}

impl SignedConsignmentProof {
    /// 动作。
    pub const fn action(&self) -> DispositionAction {
        self.action
    }

    /// 真实证明观察身份。
    pub const fn observation_id(&self) -> Uuid {
        self.observation_id
    }

    /// 验证器版本。
    pub fn validator_version(&self) -> &str {
        &self.validator_version
    }

    /// 证明摘要。
    pub const fn proof_digest(&self) -> [u8; 32] {
        self.proof_digest
    }
}

#[cfg(test)]
mod tests {
    use super::disposition_proof_observation_id;

    #[test]
    fn proof_observation_id_is_stable_and_content_bound() {
        let unsigned = disposition_proof_observation_id([9; 32], false);
        assert_eq!(unsigned, disposition_proof_observation_id([9; 32], false));
        assert_ne!(unsigned, disposition_proof_observation_id([10; 32], false));
        assert!(!unsigned.is_nil());
    }

    #[test]
    fn proof_observation_id_separates_signed_and_unsigned_proofs() {
        let unsigned = disposition_proof_observation_id([9; 32], false);
        let signed = disposition_proof_observation_id([9; 32], true);
        assert_ne!(unsigned, signed);
        assert_eq!(signed, disposition_proof_observation_id([9; 32], true));
    }
}
