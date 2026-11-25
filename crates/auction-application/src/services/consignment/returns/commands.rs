//! 接管、Take 起写与消费：组装端口命令，并在起写首次提交时铸造一次性 POST 许可。

use auction_domain::consignment::returns::ReturnIntakeFacts;
use uuid::Uuid;

use super::ReturnService;
use super::encoding;
use crate::ports::consignment::{
    AcquireReceipt, AcquireReturn, ConsignmentError, ConsumeReceipt, ConsumeReturn, PostPermit,
    StartReturnTake, WriteStart, WriteStartOutcome,
};

/// Take 起写的输入：执行者当前领取的身份与写前冻结事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartTakeInput {
    /// 接管 ID。
    pub intake_id: Uuid,
    /// 唯一 Take 工作。
    pub work_id: Uuid,
    /// 该工作的 Take 投递。
    pub delivery_id: Uuid,
    /// 本次 attempt。
    pub attempt_id: Uuid,
    /// 当前 fence。
    pub fence: u64,
    /// 当前 claim token。
    pub claim_token: Uuid,
    /// 证据会话绑定摘要。
    pub session_binding_digest: [u8; 32],
    /// 写前冻结清单摘要。
    pub expected_manifest_digest: [u8; 32],
}

/// 消费的输入：引用原起写 attempt 与精确 Take 证明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumeReturnInput {
    /// 接管 ID。
    pub intake_id: Uuid,
    /// 原起写 attempt。
    pub original_attempt_id: Uuid,
    /// 原起写 fence。
    pub fence: u64,
    /// 证明观察。
    pub observation_id: Uuid,
    /// 验证器版本。
    pub validator_version: String,
    /// 证明摘要。
    pub proof_digest: [u8; 32],
}

impl ReturnService {
    /// 按 RTS 的 K 接管；命令 ID 只由 K 派生，同一封信重复到达得到原回执。
    pub async fn acquire(
        &self,
        facts: ReturnIntakeFacts,
    ) -> Result<AcquireReceipt, ConsignmentError> {
        let (command_id, request_hash) = encoding::acquire(&facts);
        self.store.acquire_return(AcquireReturn { command_id, request_hash, facts }).await
    }

    /// 提交 Take 起写；只有首次提交返回许可，之后任何 fence、命令 ID 与重放都只得到“已提交”。
    pub async fn start_take(&self, input: StartTakeInput) -> Result<WriteStart, ConsignmentError> {
        let (command_id, request_hash) = encoding::start_take(&input);
        let outcome = self
            .store
            .start_return_take(StartReturnTake {
                command_id,
                request_hash,
                intake_id: input.intake_id,
                work_id: input.work_id,
                delivery_id: input.delivery_id,
                attempt_id: input.attempt_id,
                fence: input.fence,
                claim_token: input.claim_token,
                session_binding_digest: input.session_binding_digest,
                expected_manifest_digest: input.expected_manifest_digest,
            })
            .await?;
        Ok(match outcome {
            WriteStartOutcome::FirstCommitted(receipt) => {
                // 回执必须指回本次请求的工作与 fence；对不上就不发许可，宁可不写也不误发。
                if receipt.work_id != input.work_id || receipt.fence != input.fence {
                    return Err(ConsignmentError::Unavailable);
                }
                let permit = PostPermit::new(input.work_id, input.delivery_id, input.fence);
                WriteStart::FirstCommitted { receipt, permit }
            }
            WriteStartOutcome::AlreadyCommitted(receipt) => WriteStart::AlreadyCommitted(receipt),
        })
    }

    /// 证明 + 消费 + 资金回补；证据不足时存储保留核对并返回 `EvidenceUnproven`。
    pub async fn consume(
        &self,
        input: ConsumeReturnInput,
    ) -> Result<ConsumeReceipt, ConsignmentError> {
        let (command_id, request_hash) = encoding::consume(&input);
        self.store
            .consume_return(ConsumeReturn {
                command_id,
                request_hash,
                intake_id: input.intake_id,
                original_attempt_id: input.original_attempt_id,
                fence: input.fence,
                observation_id: input.observation_id,
                validator_version: input.validator_version,
                proof_digest: input.proof_digest,
            })
            .await
    }
}
