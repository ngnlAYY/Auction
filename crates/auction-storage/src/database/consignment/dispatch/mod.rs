//! 寄售派发轮次、尝试、证据与首派的存储适配。
//!
//! 实现 `auction_application::consignment::DispatchStore` 的五个写操作（含买家确领原件 `prove_handoff`），以及 [`DispatchWorkRepository`] 对
//! `DispatchWorkStore` 的派发 / 补寄工作来源（领取、发送请求读口、补寄待办读口、补寄新轮投递备好），
//! 并提供购买事务内的首轮原语
//! [`create_first_round_in_tx`]。尝试与 Sent MID 绑定复用拍卖的 `delivery_send_attempts` /
//! `delivery_sent_bindings`（裁定 R5-C4）：寄售只加轮、冻结身份、追加证据与尝试登记。
//! 每个写操作一个顶层事务：先取命令锁，再开 Serializable，序列化冲突有界重试，
//! 提交结果未知时调用方以同一命令 ID 重放读取原回执。

mod attempt;
mod authorize;
mod cash_funding;
mod claim;
pub(in crate::database) mod claim_proof;
mod collected_rounds;
mod downstream_round;
mod evidence;
mod first_dispatch;
mod handoff;
pub(crate) mod lease;
mod ledger;
pub(in crate::database::consignment) mod lock_order;
mod redispatch;
mod round;
mod send_request;
mod sent_binding;
#[cfg(feature = "test-db")]
mod test_access;
mod withdrawal_round;
mod work_repository;

use auction_application::consignment::{
    AttemptReceipt, AuthorizeDispatch, ConsignmentFuture, DispatchStore, EvidenceReceipt,
    HandoffReceipt, MarkWriteStarted, ProveHandoff, RecordEvidence, RoundReceipt, StartAttempt,
    WriteStartOutcome,
};
use auction_application::settlement::SettlementInternalEvidenceSnapshot;

pub use collected_rounds::CollectedRoundRepository;
pub use downstream_round::{
    DownstreamDispatchContentSource, DownstreamDispatchRequest, DownstreamRoundRepository,
    PreparedDownstreamRound,
};
pub(crate) use first_dispatch::create_first_round_in_tx;
pub(crate) use withdrawal_round::create_withdrawal_round_in_tx;
pub use work_repository::DispatchWorkRepository;

use self::attempt::{mark_write_started_in_tx, start_attempt_in_tx};
use self::authorize::authorize_in_tx;
use self::cash_funding::WriteStartInput;
use self::evidence::record_evidence_in_tx;
use self::handoff::prove_handoff_in_tx;
use self::lock_order::{pre_read_by_contract, pre_read_by_delivery};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};

impl DispatchStore for DatabaseStore {
    fn authorize_dispatch(
        &self,
        command: AuthorizeDispatch,
    ) -> ConsignmentFuture<'_, RoundReceipt> {
        Box::pin(async move {
            let identity = pre_read_by_contract(self, command.key.contract_id)
                .await
                .map_err(WriteError::into_application)?;
            let keys = LockKeys {
                command_id: command.command_id.as_uuid(),
                sellers: &identity.sellers,
                equipment: &identity.equipment,
            };
            with_deposit_transaction(self, keys, &[], &command, |command, tx| {
                Box::pin(authorize_in_tx(command, tx))
            })
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }

    fn start_attempt(&self, command: StartAttempt) -> ConsignmentFuture<'_, AttemptReceipt> {
        Box::pin(async move {
            let keys = command_lock(command.command_id.as_uuid());
            with_deposit_transaction(self, keys, &[], &command, |command, tx| {
                Box::pin(start_attempt_in_tx(command, tx))
            })
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }

    fn mark_write_started(
        &self,
        command: MarkWriteStarted,
    ) -> ConsignmentFuture<'_, WriteStartOutcome> {
        Box::pin(write_start(self, WriteStartInput { command, evidence: None }))
    }

    fn mark_cash_write_started<'a>(
        &'a self,
        command: MarkWriteStarted,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> ConsignmentFuture<'a, WriteStartOutcome> {
        Box::pin(write_start(self, WriteStartInput { command, evidence: Some(evidence) }))
    }

    fn record_evidence(&self, command: RecordEvidence) -> ConsignmentFuture<'_, EvidenceReceipt> {
        Box::pin(async move {
            let keys = command_lock(command.command_id.as_uuid());
            with_deposit_transaction(self, keys, &[], &command, |command, tx| {
                Box::pin(record_evidence_in_tx(command, tx))
            })
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }

    fn prove_handoff(&self, command: ProveHandoff) -> ConsignmentFuture<'_, HandoffReceipt> {
        Box::pin(async move {
            // 确领改 position、挂单与占用：与购买、起写同取卖家与规范实物会话锁（D9 §1.3）。
            let identity = pre_read_by_contract(self, command.key.contract_id)
                .await
                .map_err(WriteError::into_application)?;
            let keys = LockKeys {
                command_id: command.command_id.as_uuid(),
                sellers: &identity.sellers,
                equipment: &identity.equipment,
            };
            with_deposit_transaction(self, keys, &[], &command, |command, tx| {
                Box::pin(prove_handoff_in_tx(command, tx))
            })
            .await
            .map_err(WriteError::into_application)?
            .into_result()
        })
    }
}

/// 起写事务：现金腿的资金证据随输入借入，同一事务重试沿用同一份证据。
async fn write_start(
    store: &DatabaseStore,
    input: WriteStartInput<'_>,
) -> Result<WriteStartOutcome, auction_application::consignment::ConsignmentError> {
    let identity = pre_read_by_delivery(store, input.command.attempt.delivery_id)
        .await
        .map_err(WriteError::into_application)?;
    let keys = LockKeys {
        command_id: input.command.command_id.as_uuid(),
        sellers: &identity.sellers,
        equipment: &identity.equipment,
    };
    with_deposit_transaction(store, keys, &[], &input, |input, tx| {
        Box::pin(mark_write_started_in_tx(input, tx))
    })
    .await
    .map_err(WriteError::into_application)?
    .into_result()
}

/// 登记尝试与追加证据只需全局命令锁；授权与起写另取卖家与规范实物会话锁（见 `lock_order`）。
const fn command_lock(command_id: uuid::Uuid) -> LockKeys<'static> {
    LockKeys { command_id, sellers: &[], equipment: &[] }
}
