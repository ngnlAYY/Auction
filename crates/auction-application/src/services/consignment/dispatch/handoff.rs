//! 证明买家已确领原件：组装 `prove_handoff` 端口命令（系统命令 ID 与规范摘要由本层派生）。
//!
//! 保护不变量 1、6：命令 ID 只由原物义务加原轮决定，同一轮的确领只能有一份结论；所消费的证据身份与摘要
//! 进摘要，换证据即换请求（同 ID 异请求冲突）。观察不会自动成为确领结论，验证器签收与付款认证由存储在锁内判定。

use auction_domain::consignment::dispatch::{AttemptKey, DispatchRoundKey};

use super::DispatchService;
use super::encoding;
use crate::ports::consignment::{ConsignmentError, HandoffReceipt, ProveHandoff};

/// 确领证明的输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProveHandoffInput {
    /// 原件腿的轮业务键（main 原件 COD 腿、isekai 原件交割腿）。
    pub key: DispatchRoundKey,
    /// 该轮已起写的尝试。
    pub attempt: AttemptKey,
    /// 已追加的 `original_claim` 证据的远端观察 ID。
    pub remote_observation_id: String,
    /// 该证据的摘要。
    pub evidence_digest: [u8; 32],
}

impl DispatchService {
    /// 证明买家已确领原件；付款已认证时同事务放行卖款义务。
    pub async fn prove_handoff(
        &self,
        input: ProveHandoffInput,
    ) -> Result<HandoffReceipt, ConsignmentError> {
        let (command_id, request_hash) = encoding::prove_handoff(
            &input.key,
            &input.attempt,
            &input.remote_observation_id,
            &input.evidence_digest,
        );
        self.store
            .prove_handoff(ProveHandoff {
                command_id,
                request_hash,
                key: input.key,
                attempt: input.attempt,
                remote_observation_id: input.remote_observation_id,
                evidence_digest: input.evidence_digest,
            })
            .await
    }
}
