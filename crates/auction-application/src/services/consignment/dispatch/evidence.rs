//! 追加派发证据：只追加；同键同摘要重放，同键异摘要由账本或证据唯一键判冲突。

use auction_domain::consignment::dispatch::{AttemptKey, EvidenceCategory};
use auction_time::Timestamp;

use super::DispatchService;
use super::encoding::{self, EvidenceFacts};
use crate::ports::consignment::{ConsignmentError, EvidenceReceipt, RecordEvidence};

/// 追加证据的输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordEvidenceInput {
    /// 尝试键。
    pub attempt: AttemptKey,
    /// 证据类别。
    pub category: EvidenceCategory,
    /// 远端观察 ID；Sent 绑定类即 Sent MID。
    pub remote_observation_id: String,
    /// 观察时刻。
    pub observed_at: Timestamp,
    /// 验证器版本。
    pub validator_version: String,
    /// 证据摘要。
    pub evidence_digest: [u8; 32],
}

impl DispatchService {
    /// 追加一条证据；观察不会自动成为有效结论。
    pub async fn record_evidence(
        &self,
        input: RecordEvidenceInput,
    ) -> Result<EvidenceReceipt, ConsignmentError> {
        let (command_id, request_hash) = encoding::record_evidence(&EvidenceFacts {
            attempt: &input.attempt,
            category: input.category,
            remote_observation_id: &input.remote_observation_id,
            observed_at: input.observed_at,
            validator_version: &input.validator_version,
            evidence_digest: &input.evidence_digest,
        });
        self.store
            .record_evidence(RecordEvidence {
                command_id,
                request_hash,
                attempt: input.attempt,
                category: input.category,
                remote_observation_id: input.remote_observation_id,
                observed_at: input.observed_at,
                validator_version: input.validator_version,
                evidence_digest: input.evidence_digest,
            })
            .await
    }
}
