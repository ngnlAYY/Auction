//! 派发写端口。

use super::commands::{
    AttemptReceipt, AuthorizeDispatch, EvidenceReceipt, HandoffReceipt, MarkWriteStarted,
    ProveHandoff, RecordEvidence, RoundReceipt, StartAttempt,
};
use crate::ports::consignment::deposit::WriteStartOutcome;
use crate::ports::consignment::errors::{ConsignmentError, ConsignmentFuture};
use crate::ports::settlement::SettlementInternalEvidenceSnapshot;

/// 派发写端；每个方法对应一个事务，同命令 ID 同摘要返回原结果，同 ID 异摘要冲突。
pub trait DispatchStore: Send + Sync {
    /// 建首轮或合法新轮；新轮必须引用封闭上一轮的退件接管。
    fn authorize_dispatch(&self, command: AuthorizeDispatch)
    -> ConsignmentFuture<'_, RoundReceipt>;

    /// 冻结身份并登记尝试；冻结身份与轮不一致即拒绝。
    fn start_attempt(&self, command: StartAttempt) -> ConsignmentFuture<'_, AttemptReceipt>;

    /// 提交起写。只报告本次是否首次提交（复用第 4 期 `WriteStartOutcome`）：回执里
    /// `decision_id` 载轮 ID、`work_id` 载投递 ID。许可只能由应用服务据此构造并以值移动一次。
    fn mark_write_started(
        &self,
        command: MarkWriteStarted,
    ) -> ConsignmentFuture<'_, WriteStartOutcome>;

    /// 现金腿（载体 1、卖款 S、退款 P）提交起写：与 [`Self::mark_write_started`] 同一命令与同一回执，另把发信
    /// 会话里取得的新鲜资金证据借给起写事务，由资金 owner 按拍卖通用现金门的口径在锁内核对；不满足即整笔回滚、
    /// 不发许可（不变量 4、6）。证据不进请求摘要：它只决定首次提交能否成立，重放仍按原回执回答。
    ///
    /// 缺省实现失败关闭：不支持现金证据的写端不得替现金腿起写。
    fn mark_cash_write_started<'a>(
        &'a self,
        _command: MarkWriteStarted,
        _evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> ConsignmentFuture<'a, WriteStartOutcome> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    /// 追加证据；只追加，同 `(attempt, 类别, 远端观察)` 重放返回原结果。
    fn record_evidence(&self, command: RecordEvidence) -> ConsignmentFuture<'_, EvidenceReceipt>;

    /// 证明买家已确领原件：同事务写确领证明、position 已交付、释放实物占用、挂单成交关闭，并在付款已认证时
    /// 放行卖款义务（D9 §4.2 第 3 步）。缺证、未签收或付款未认证时整笔回滚且不占命令 ID。
    ///
    /// 缺省实现失败关闭：不支持确领证明的写端不得放行卖款。
    fn prove_handoff(&self, _command: ProveHandoff) -> ConsignmentFuture<'_, HandoffReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }
}
