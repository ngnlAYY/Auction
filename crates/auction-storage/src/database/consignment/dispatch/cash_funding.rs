//! 派发轮现金腿（isekai 载体本金 1、卖款 S、退款 P）的资金占用接线：起写同事务 reserve → in_flight，
//! Sent 绑定同事务 spent（D9 §4.3–4.4）。
//!
//! 资金规则与写序只在资金 owner（`settlement::funding::consignment::cash_legs`）；本模块只决定派发事务里
//! 的调用时机与锁位置：资金锁前缀（funding worlds → UID 账户）在任何业务行锁之前，现金义务行锁在受益用户
//! 之后、投递行之前，与通用现金起写同序（D9 §1.3）。原件腿与原件交割腿没有现金，不经本模块。
//!
//! 保护不变量 2、4、6：资金失败一律整笔回滚，不写账本回执、不发起写许可、零外写。起写事务不用保存点，
//! 已写入一半的资金行无法与确定拒绝回执同提交；余额不足、账户未就绪属于暂不可用，不占死该次命令 ID
//! （D9 §1.3“临时缺证、暂不可用不占死未来系统命令”）。
//!
//! 现金外发另要求发信会话里取得的新鲜资金证据（维护者 2026-11-25 决定）：证据必须绑定本次尝试、本投递的
//! 当前租约与冻结身份摘要、起写锁验的同一凭据与轮的世界；缺证据或绑定不符同样整笔回滚、零许可。余额与
//! UID 的核对只在资金 owner 的通用现金门里做，本模块不复制。

use auction_application::consignment::MarkWriteStarted;
use auction_application::core::RequestHash;
use auction_application::settlement::{
    CashLeg, FundingError, FundingMutation, InternalEvidencePurpose,
    SettlementInternalEvidenceSnapshot,
};
use auction_domain::CommandId;
use auction_domain::consignment::dispatch::dispatch_attempt_id;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::database::MailEvidenceTransactionError;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::returns::funding_error;
use crate::database::delivery::scheduler::entitlement::lock_send_beneficiary;
use crate::database::settlement::funding::consignment::cash_legs::{
    DispatchCash, lock_dispatch_funding_in_tx, settle_dispatch_cash_claimed_in_tx,
    settle_dispatch_cash_sent_in_tx, start_dispatch_cash_in_tx,
};

/// 派发义务对应的现金腿；原件、原件交割与退回类义务没有现金，返回 `None`。义务种类不可变，无锁读取。
pub(super) async fn cash_leg_of(
    connection: &mut PgConnection,
    obligation_id: Uuid,
) -> Result<Option<CashLeg>, WriteError> {
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT obligation_kind FROM public.settlement_obligations WHERE id = $1",
    )
    .bind(obligation_id)
    .fetch_optional(connection)
    .await?;
    Ok(match kind.as_deref() {
        Some("winner_payment_carrier_cod") => Some(CashLeg::CarrierPrincipal),
        Some("seller_payout") => Some(CashLeg::Seller),
        Some("buyer_refund") => Some(CashLeg::Refund),
        _ => None,
    })
}

/// 资金锁前缀：在挂单、position、凭据与投递行锁之前取得 funding worlds 与成交 UID 账户锁。
pub(super) async fn lock_funding_prefix(
    tx: &mut DepositTransaction<'_, '_>,
    contract_id: Uuid,
) -> Result<(), WriteError> {
    lock_dispatch_funding_in_tx(tx.tracked(), contract_id).await.map_err(funding_error)
}

/// 受益用户之后、投递之前锁现金义务行（D9 全局顺序 `users → obligations → allocations → deliveries`），
/// 资金迁移随后在同一把义务锁下复核轮与金额。受益用户的 `FOR SHARE` 在收件人授权里会再取一次，同事务内无副作用。
pub(super) async fn lock_obligation(
    connection: &mut PgConnection,
    obligation_id: Uuid,
) -> Result<(), WriteError> {
    lock_send_beneficiary(&mut *connection, obligation_id).await?;
    sqlx::query("SELECT 1 FROM public.settlement_obligations WHERE id = $1 FOR UPDATE")
        .bind(obligation_id)
        .fetch_optional(connection)
        .await?;
    Ok(())
}

/// 一次派发现金腿的资金调用：派发命令的身份、合同 / 义务 / 投递、腿与轮冻结金额。
pub(super) struct CashSend {
    pub(super) command_id: CommandId,
    pub(super) request_hash: RequestHash,
    pub(super) contract_id: Uuid,
    pub(super) obligation_id: Uuid,
    pub(super) delivery_id: Uuid,
    pub(super) leg: CashLeg,
    pub(super) amount: i64,
}

impl CashSend {
    /// 派发命令在资金账本里的身份：沿用应用层派生的系统命令 ID 与请求摘要。
    fn dispatch(&self) -> DispatchCash {
        DispatchCash {
            mutation: FundingMutation {
                command_id: self.command_id.as_uuid(),
                request_digest: self.request_hash.as_bytes(),
            },
            contract_id: self.contract_id,
            obligation_id: self.obligation_id,
            delivery_id: self.delivery_id,
            leg: self.leg,
            amount: self.amount,
        }
    }
}

/// 起写事务的输入：起写命令与现金腿在发信会话里取得的资金证据（只借用到事务结束，不进摘要）。
pub(super) struct WriteStartInput<'a> {
    pub(super) command: MarkWriteStarted,
    pub(super) evidence: Option<&'a SettlementInternalEvidenceSnapshot>,
}

/// 首次起写同事务：核对同会话资金证据后，现金腿 reserve → in_flight；返回 `false` 表示现金门不放行。
///
/// 缺证据或证据绑定不符按暂不可用整笔回滚（不占死命令 ID、零许可）；余额与 UID 由资金 owner 的通用现金门
/// 在同一账户锁下核对。现金门不放行时本事务只留下通用现金门写入的账户冻结，调用方提交它、不写起写事实与
/// 账本回执、不发许可（不变量 2、4、6）。
pub(super) async fn start(
    tx: &mut DepositTransaction<'_, '_>,
    send: &CashSend,
    command: &MarkWriteStarted,
    evidence: Option<&SettlementInternalEvidenceSnapshot>,
) -> Result<bool, WriteError> {
    let Some(evidence) = evidence else {
        tracing::warn!(
            code = "dispatch_cash_evidence_missing",
            "寄售现金腿缺同会话资金证据，不起写"
        );
        return Err(funding_error(FundingError::Unproven));
    };
    check_evidence_binding(tx.connection(), command, evidence).await?;
    start_dispatch_cash_in_tx(tx.tracked(), &send.dispatch(), evidence).await.map_err(funding_error)
}

/// 证据必须是为本次尝试、本投递当前租约与冻结身份摘要取得的，会话凭据与起写锁验的绑定相同、世界与轮相同，
/// 且凭据行仍认可这份观察（与拍卖现金起写同一凭据证据锁）。
async fn check_evidence_binding(
    connection: &mut PgConnection,
    command: &MarkWriteStarted,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<(), WriteError> {
    let delivery_id = command.attempt.delivery_id;
    let (world_id, request_digest): (String, Vec<u8>) = sqlx::query_as(
        "SELECT round.world_id, delivery.request_digest
         FROM public.consignment_dispatch_rounds AS round
         JOIN public.deliveries AS delivery ON delivery.id = round.delivery_id
         WHERE round.delivery_id = $1",
    )
    .bind(delivery_id)
    .fetch_one(&mut *connection)
    .await?;
    let claim_fencing = i64::try_from(command.attempt.fence)?;
    let request_hash = <[u8; 32]>::try_from(request_digest.as_slice()).ok().map(RequestHash::new);
    let purpose = request_hash.map(|request_hash| InternalEvidencePurpose::DeliveryClaim {
        delivery_id,
        claim_token: command.claim_token,
        claim_fencing,
        request_hash,
    });
    let same_credential = command.credential.as_ref().is_some_and(|credential| {
        credential.credential_id == evidence.identity.credential_id
            && credential.credential_version == evidence.identity.credential_version
            && credential.runtime_generation == evidence.identity.runtime.generation
    });
    if evidence.operation_id != dispatch_attempt_id(&command.attempt)
        || purpose.as_ref() != Some(&evidence.purpose)
        || !same_credential
        || evidence.identity.world.as_str() != world_id
    {
        tracing::warn!(
            code = "dispatch_cash_evidence_mismatch",
            "寄售现金腿资金证据绑定不符，不起写"
        );
        return Err(funding_error(FundingError::Unproven));
    }
    match crate::lock_mail_evidence_credentials(connection, evidence).await {
        Ok(()) => Ok(()),
        Err(MailEvidenceTransactionError::Sqlx(error)) => Err(error.into()),
        Err(MailEvidenceTransactionError::Rejected(_reason)) => {
            tracing::warn!(
                code = "dispatch_cash_evidence_stale",
                "寄售现金腿资金证据已失效，不起写"
            );
            Err(funding_error(FundingError::Unproven))
        }
    }
}

/// Sent 绑定同事务：现金腿占用转 spent（已证外达）。
pub(super) async fn settle_sent(
    tx: &mut DepositTransaction<'_, '_>,
    send: &CashSend,
) -> Result<(), WriteError> {
    settle_dispatch_cash_sent_in_tx(tx.tracked(), &send.dispatch()).await.map_err(funding_error)
}

/// 现金确领证据同事务：已支出的卖款 / 退款追加 claimed（PIWP，D9 §2.3 `remote_cash_claimed`）。
pub(super) async fn settle_claimed(
    tx: &mut DepositTransaction<'_, '_>,
    send: &CashSend,
) -> Result<(), WriteError> {
    settle_dispatch_cash_claimed_in_tx(tx.tracked(), &send.dispatch()).await.map_err(funding_error)
}
