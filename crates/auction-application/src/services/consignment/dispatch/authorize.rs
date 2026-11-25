//! 授权轮、登记尝试与提交起写：组装端口命令，并在首次提交时铸造一次性 POST 许可。

use auction_domain::consignment::dispatch::{
    AttemptKey, DispatchRoundKey, FrozenDispatchIdentity, RoundOrigin, dispatch_round_id,
};
use uuid::Uuid;

use super::DispatchService;
use super::encoding;
use crate::ports::consignment::{
    AttemptReceipt, AuthorizeDispatch, ConsignmentError, MarkWriteStarted, PostPermit,
    RoundReceipt, StartAttempt, WriteStart, WriteStartOutcome,
};
use crate::ports::delivery::DeliverySendCredentialBinding;
use crate::ports::settlement::SettlementInternalEvidenceSnapshot;

impl DispatchService {
    /// 授权首轮或合法新轮；新轮必须引用已封闭上一轮的退件接管。
    pub async fn authorize(
        &self,
        key: DispatchRoundKey,
        origin: RoundOrigin,
        delivery_id: Uuid,
        frozen: FrozenDispatchIdentity,
    ) -> Result<RoundReceipt, ConsignmentError> {
        let (command_id, request_hash) = encoding::authorize(&key, &origin, delivery_id, &frozen);
        self.store
            .authorize_dispatch(AuthorizeDispatch {
                command_id,
                request_hash,
                key,
                origin,
                delivery_id,
                frozen,
            })
            .await
    }

    /// 登记并冻结一次尝试；冻结身份与轮不一致即拒绝。
    pub async fn start_attempt(
        &self,
        key: DispatchRoundKey,
        attempt: AttemptKey,
        claim_token: Uuid,
        frozen: FrozenDispatchIdentity,
    ) -> Result<AttemptReceipt, ConsignmentError> {
        let round_id = dispatch_round_id(&key);
        let (command_id, request_hash) =
            encoding::start_attempt(round_id, &attempt, claim_token, &frozen);
        self.store
            .start_attempt(StartAttempt {
                command_id,
                request_hash,
                round_id,
                attempt,
                claim_token,
                frozen,
            })
            .await
    }

    /// 提交起写；只有首次提交返回许可，之后任何 fence、命令 ID 与重放都只得到“已提交”。
    pub async fn mark_write_started(
        &self,
        attempt: AttemptKey,
        claim_token: Uuid,
        credential: Option<DeliverySendCredentialBinding>,
    ) -> Result<WriteStart, ConsignmentError> {
        self.write_start(attempt, claim_token, credential, None).await
    }

    /// 现金腿（载体 1、卖款 S、退款 P）提交起写：命令与摘要同 [`Self::mark_write_started`]，另把发信会话里
    /// 取得的新鲜资金证据借给起写事务核对（不进摘要）；证据不满足时存储整笔回滚、不发许可（不变量 4、6）。
    pub async fn mark_cash_write_started(
        &self,
        attempt: AttemptKey,
        claim_token: Uuid,
        credential: Option<DeliverySendCredentialBinding>,
        evidence: &SettlementInternalEvidenceSnapshot,
    ) -> Result<WriteStart, ConsignmentError> {
        self.write_start(attempt, claim_token, credential, Some(evidence)).await
    }

    async fn write_start(
        &self,
        attempt: AttemptKey,
        claim_token: Uuid,
        credential: Option<DeliverySendCredentialBinding>,
        evidence: Option<&SettlementInternalEvidenceSnapshot>,
    ) -> Result<WriteStart, ConsignmentError> {
        let (command_id, request_hash) =
            encoding::mark_write_started(&attempt, claim_token, credential.as_ref());
        let command =
            MarkWriteStarted { command_id, request_hash, attempt, claim_token, credential };
        let outcome = match evidence {
            Some(evidence) => self.store.mark_cash_write_started(command, evidence).await?,
            None => self.store.mark_write_started(command).await?,
        };
        Ok(match outcome {
            WriteStartOutcome::FirstCommitted(receipt) => {
                // 回执必须指回本次请求的投递与 fence；对不上就不发许可，宁可不写也不误发。
                if receipt.work_id != attempt.delivery_id || receipt.fence != attempt.fence {
                    return Err(ConsignmentError::Unavailable);
                }
                let permit = PostPermit::new(receipt.work_id, attempt.delivery_id, receipt.fence);
                WriteStart::FirstCommitted { receipt, permit }
            }
            WriteStartOutcome::AlreadyCommitted(receipt) => WriteStart::AlreadyCommitted(receipt),
        })
    }
}
