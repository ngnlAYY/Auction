//! 尝试登记与首次起写：冻结 Send 身份，持久提交 `write_started` 之后才可 POST。
//!
//! 保护不变量 2、4、5：每个投递至多一条成功起写（`delivery_send_attempts` 的存在即起写事实），
//! 起写事务在投递行锁下复验当前 claim、fence 与锁后时钟，旧 fence 与过租只得到确定拒绝，
//! 已起写的投递无论新 fence、新命令 ID 还是重放都只得到“已提交”，永远不再产生 POST 许可。
//! 锁序照 D9 §1.3 / R5 §3.8 与全局顺序：命令锁与规范实物会话锁之后，先锁挂单与 position，再锁
//! active 凭据、受益用户、投递，最后锁履约行；写序与既有拍卖的 `persist_sending` 同构：`claimed→sending` → 追加冻结尝试 →
//! 审计。寄售另在同事务登记 `consignment_dispatch_attempts`，原件腿并把 position 推进到 `in_transit`。

use auction_application::consignment::{
    AttemptReceipt, MarkWriteStarted, StartAttempt, StartReceipt, WriteStartOutcome,
};
use auction_application::settlement::MailRuntimeBinding;
use auction_domain::CommandId;
use auction_domain::consignment::dispatch::{
    DispatchClaimFacts, WriteStartDecision, WriteStartFacts, WriteStartRejection,
    decide_write_start, dispatch_attempt_id,
};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::cash_funding::{self, CashSend, WriteStartInput};
use super::downstream_round::downstream_ready;
use super::first_dispatch::hex;
use super::ledger::{
    Arbitration, LedgerCommand, Rejection, Step, TargetKind, arbitrate, insert_rejection,
    insert_success,
};
use super::lock_order::{lock_listing_and_position, read_by_delivery};
use super::round::{StoredRound, load_by_delivery, load_by_round_id};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::reads::orders::record_order_stage_change_in_tx;
use crate::database::delivery::audit::{
    DeliveryAudit, START_SENDING, append as append_delivery_audit, worker_state_metadata,
};
use crate::database::delivery::scheduler::attempts::lock_credential_binding;
use crate::database::delivery::scheduler::entitlement::{
    authorize_consignment_send, lock_send_beneficiary,
};
use crate::database::{EntitlementDecision, MailEvidenceTransactionError};

/// 投递行在锁下读到的领取事实与冻结载荷。
struct DeliveryRow {
    status: String,
    lease_token: Option<Uuid>,
    fencing_version: i64,
    lease_live: bool,
    recipient: String,
    payload: Value,
    attempt_count: i32,
}

const PLAIN_DELIVERY: &str = "SELECT status, lease_token, fencing_version,
        COALESCE(lease_expires_at > clock_timestamp(), false) AS lease_live,
        recipient, payload, attempt_count
     FROM public.deliveries WHERE id = $1";
const LOCKED_DELIVERY: &str = "SELECT status, lease_token, fencing_version,
        COALESCE(lease_expires_at > clock_timestamp(), false) AS lease_live,
        recipient, payload, attempt_count
     FROM public.deliveries WHERE id = $1 FOR UPDATE";

/// 读取投递；`lock` 为真时加行锁（起写），否则只读（登记尝试）。
async fn load_delivery(
    connection: &mut PgConnection,
    delivery_id: Uuid,
    lock: bool,
) -> Result<Option<DeliveryRow>, WriteError> {
    let sql = if lock { LOCKED_DELIVERY } else { PLAIN_DELIVERY };
    let Some(row) = sqlx::query(sql).bind(delivery_id).fetch_optional(connection).await? else {
        return Ok(None);
    };
    Ok(Some(DeliveryRow {
        status: row.try_get("status")?,
        lease_token: row.try_get("lease_token")?,
        fencing_version: row.try_get("fencing_version")?,
        lease_live: row.try_get("lease_live")?,
        recipient: row.try_get("recipient")?,
        payload: row.try_get("payload")?,
        attempt_count: row.try_get("attempt_count")?,
    }))
}

impl DeliveryRow {
    /// 只有 `claimed` 才有可用于起写的领取；其他状态按“没有当前领取”处理。
    fn claim_facts(&self) -> Result<DispatchClaimFacts, WriteError> {
        if self.status != "claimed" {
            return Ok(DispatchClaimFacts {
                current_fence: 0,
                current_claim_token: Uuid::nil(),
                lease_live: false,
            });
        }
        Ok(DispatchClaimFacts {
            current_fence: u64::try_from(self.fencing_version)?,
            current_claim_token: self.lease_token.unwrap_or_else(Uuid::nil),
            lease_live: self.lease_live,
        })
    }

    /// 载荷里冻结的身份摘要。
    fn identity_digest(&self) -> Option<&str> {
        self.payload.get("identity_digest").and_then(Value::as_str)
    }
}

/// 义务种类对应的派发腿；未知种类失败关闭。
fn leg_for(kind: &str) -> Option<&'static str> {
    match kind {
        // 撤回件（PIWX）寄还的也是原件：与买家原件腿同一腿名，退件接管与消费沿用原件腿。
        "winner_equipment_cod" | "winner_equipment_handoff" | "withdrawn_return" => {
            Some("original")
        }
        "winner_payment_carrier_cod" => Some("carrier"),
        "seller_payout" => Some("cash_payout"),
        "buyer_refund" => Some("cash_refund"),
        _ => None,
    }
}

/// 登记并校验一次尝试：冻结身份必须与轮一致，且当前领取有效。
///
/// 本期的持久尝试行与冻结 Send 身份在起写事务里同时落库（`c6_attempt_send_fk` 要求先有冻结尝试），
/// 所以这里只核对并回执，由 `mark_write_started` 追加事实。
pub(super) async fn start_attempt_in_tx(
    command: &StartAttempt,
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Step<AttemptReceipt>, WriteError> {
    let attempt_id = dispatch_attempt_id(&command.attempt);
    let ledger = LedgerCommand {
        command_id: command.command_id.as_uuid(),
        operation: auction_application::consignment::DispatchOperation::StartAttempt,
        target: TargetKind::Round,
        target_id: command.round_id,
        digest: command.request_hash,
    };
    match arbitrate(tx.connection(), &ledger).await? {
        Step::Rejected(rejection) => return Ok(Step::Rejected(rejection)),
        Step::Done(Arbitration::Replay(_)) => {
            return Ok(Step::Done(AttemptReceipt {
                command_id: command.command_id,
                attempt_id,
                replayed: true,
            }));
        }
        Step::Done(Arbitration::First) => {}
    }
    let connection = tx.connection();
    let Some(round) = load_by_round_id(connection, command.round_id).await? else {
        return insert_rejection(connection, &ledger, Rejection::SourceMismatch).await;
    };
    let delivery_id = command.attempt.delivery_id;
    let delivery = load_delivery(connection, delivery_id, false).await?;
    let identity_hex = hex(&command.frozen.identity_digest());
    let Some(delivery) = delivery.filter(|delivery| {
        round.delivery_id == delivery_id
            && round.matches(&command.frozen)
            && delivery.identity_digest() == Some(identity_hex.as_str())
    }) else {
        // 冻结身份被改动、投递不属于该轮：不登记，不占 fence。
        return insert_rejection(connection, &ledger, Rejection::SourceMismatch).await;
    };
    let claim = delivery.claim_facts()?;
    if claim.current_fence != command.attempt.fence
        || claim.current_claim_token != command.claim_token
    {
        return insert_rejection(connection, &ledger, Rejection::FenceMismatch).await;
    }
    if !claim.lease_live {
        return insert_rejection(connection, &ledger, Rejection::LeaseLost).await;
    }
    // activate 等价点：与起写同一授权规则，保留权利时不登记尝试。
    if let Some(rejection) =
        authorize_round_recipient(connection, round.key.obligation_id, delivery_id).await?
    {
        return insert_rejection(connection, &ledger, rejection).await;
    }
    insert_success(
        connection,
        &ledger,
        "attempt_started",
        json!({
            "round_id": command.round_id,
            "delivery_id": delivery_id,
            "attempt_id": attempt_id,
            "fence": command.attempt.fence,
        }),
    )
    .await?;
    Ok(Step::Done(AttemptReceipt { command_id: command.command_id, attempt_id, replayed: false }))
}

/// 提交起写：只有首次提交的执行者可 POST；其余一律“已提交”，不得再发。现金腿另需同会话资金证据。
pub(super) async fn mark_write_started_in_tx(
    input: &WriteStartInput<'_>,
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Step<WriteStartOutcome>, WriteError> {
    let command = &input.command;
    let attempt_id = dispatch_attempt_id(&command.attempt);
    let ledger = LedgerCommand {
        command_id: command.command_id.as_uuid(),
        operation: auction_application::consignment::DispatchOperation::MarkWriteStarted,
        target: TargetKind::Attempt,
        target_id: attempt_id,
        digest: command.request_hash,
    };
    let delivery_id = command.attempt.delivery_id;
    // 锁集合与预读不同则退出重规划（D9 §1.3）。
    let current = read_by_delivery(tx.connection(), delivery_id).await?;
    tx.ensure_lock_set(&current.sellers, &current.equipment, &[])?;
    match arbitrate(tx.connection(), &ledger).await? {
        Step::Rejected(rejection) => return Ok(Step::Rejected(rejection)),
        Step::Done(Arbitration::Replay(body)) => {
            let receipt = receipt_from_body(command.command_id, &body, tx.connection()).await?;
            return Ok(Step::Done(WriteStartOutcome::AlreadyCommitted(receipt)));
        }
        Step::Done(Arbitration::First) => {}
    }
    // MoogleMail Send 必有 Cookie 凭据：没有凭据绑定的起写不可能对应一次合法发送，失败关闭、
    // 不取任何行锁（不变量 4、6）。
    let Some(credential) = command.credential.as_ref() else {
        return insert_rejection(tx.connection(), &ledger, Rejection::ActionNotAllowed).await;
    };
    // 起写在第一把行锁位置取运行时 generation（与通用现金起写同序 generation → … → 业务 →
    // 凭据 → 投递）：换代后的旧绑定一律不发许可（不变量 4、6）。
    let runtime = MailRuntimeBinding { generation: credential.runtime_generation };
    match crate::lock_mail_runtime_generation(tx.connection(), &runtime).await {
        Ok(()) => {}
        Err(MailEvidenceTransactionError::Sqlx(error)) => return Err(error.into()),
        Err(MailEvidenceTransactionError::Rejected(_reason)) => {
            return insert_rejection(tx.connection(), &ledger, Rejection::ActionNotAllowed).await;
        }
    }
    // 轮行只追加，无锁读取以确定合同与义务；之后严格按合同顺序取行锁。
    let Some(round) = load_by_delivery(tx.connection(), delivery_id).await? else {
        return insert_rejection(tx.connection(), &ledger, Rejection::SourceMismatch).await;
    };
    // 现金腿（D9 §4.3–4.4）：资金锁前缀 funding worlds → UID 账户排在 generation 之后、任何业务行锁之前，
    // 与通用现金起写的账户锁位置一致；账户缺失等资金失败整笔回滚，不写回执、不发许可（不变量 2、4、6）。
    let cash_leg = cash_funding::cash_leg_of(tx.connection(), round.key.obligation_id).await?;
    // 纵深防御：轮冻结了 Credits 而义务不是现金腿时没有资金门可走，按数据损坏失败关闭（不变量 4、6）。
    if cash_leg.is_none() && round.credits > 0 {
        return Err(WriteError::invariant("consignment_dispatch_cash_leg_missing"));
    }
    if cash_leg.is_some() {
        cash_funding::lock_funding_prefix(tx, round.key.contract_id).await?;
    }
    // 挂单 → position → active 凭据 → 受益用户 → 投递 → 履约行 → 判定：凭据先于用户与投递（通用
    // 内部执行器按“凭据 → users”加锁），用户、投递、履约行照 D9 全局顺序
    // users → obligations → allocations → deliveries → fulfillments；两个并发起写在投递行串行。
    if !lock_listing_and_position(tx.connection(), round.key.contract_id).await? {
        return insert_rejection(tx.connection(), &ledger, Rejection::SourceMismatch).await;
    }
    if !credential.validate()
        || !lock_credential_binding(tx.tracked(), &round.world_id, Some(credential)).await?
    {
        // 凭据已非 active、版本或世界不符：不发许可，也不写任何起写事实。
        return insert_rejection(tx.connection(), &ledger, Rejection::ActionNotAllowed).await;
    }
    if cash_leg.is_some() {
        cash_funding::lock_obligation(tx.connection(), round.key.obligation_id).await?;
    }
    // 下游义务（isekai 交割、卖款、退款）起写前复核已就绪（PIWP，见 `downstream_round`）：不就绪即轮来源不符，
    // 确定拒绝、零许可零占用（不变量 4、6；D9 §4.2 第 3 步、§4.3 禁双派）。
    if !downstream_ready(tx.connection(), round.key.obligation_id).await? {
        return insert_rejection(tx.connection(), &ledger, Rejection::ActionNotAllowed).await;
    }
    // 最终 Send 授权点：保留权利（改名、权利非 active、受益人不符）时不发许可、不写起写事实。
    if let Some(rejection) =
        authorize_round_recipient(tx.connection(), round.key.obligation_id, round.delivery_id)
            .await?
    {
        return insert_rejection(tx.connection(), &ledger, rejection).await;
    }
    let Some(delivery) = load_delivery(tx.connection(), delivery_id, true).await? else {
        return insert_rejection(tx.connection(), &ledger, Rejection::SourceMismatch).await;
    };
    let started: Option<(Uuid, i64)> = sqlx::query_as(
        "SELECT attempt_id, fence FROM public.consignment_dispatch_attempts
         WHERE delivery_id = $1 ORDER BY fence LIMIT 1",
    )
    .bind(delivery_id)
    .fetch_optional(tx.connection())
    .await?;
    let send_attempted: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.delivery_send_attempts WHERE delivery_id = $1)",
    )
    .bind(delivery_id)
    .fetch_one(tx.connection())
    .await?;
    let decision = decide_write_start(&WriteStartFacts {
        already_started: send_attempted,
        claim: delivery.claim_facts()?,
        request_fence: command.attempt.fence,
        request_claim_token: command.claim_token,
    });
    match decision {
        WriteStartDecision::Reject(rejection) => {
            let rejection = match rejection {
                WriteStartRejection::FenceMismatch => Rejection::FenceMismatch,
                WriteStartRejection::LeaseLost => Rejection::LeaseLost,
            };
            insert_rejection(tx.connection(), &ledger, rejection).await
        }
        WriteStartDecision::Already => {
            // 已有起写但没有寄售尝试行（不经本端口的起写）：无法给出原 attempt，保守地不发许可。
            let Some((existing_attempt, existing_fence)) = started else {
                return Err(WriteError::invariant("consignment_dispatch_attempt_missing"));
            };
            let fence = u64::try_from(existing_fence)?;
            insert_success(
                tx.connection(),
                &ledger,
                "write_started",
                json!({
                    "delivery_id": delivery_id,
                    "attempt_id": existing_attempt,
                    "fence": fence,
                }),
            )
            .await?;
            let receipt = start_receipt(command.command_id, &round, existing_attempt, fence);
            Ok(Step::Done(WriteStartOutcome::AlreadyCommitted(receipt)))
        }
        WriteStartDecision::First => {
            if let Some(leg) = cash_leg {
                // 现金腿（载体本金 1、卖款 S、退款 P）在起写同事务 reserve → in_flight：占用提交在先、POST
                // 在后；余额不足或占用状态不符整笔回滚，零起写事实、零许可（D9 §4.3–4.4，不变量 2、4、6）。
                let send = CashSend {
                    command_id: command.command_id,
                    request_hash: command.request_hash,
                    contract_id: round.key.contract_id,
                    obligation_id: round.key.obligation_id,
                    delivery_id,
                    leg,
                    amount: round.credits,
                };
                if !cash_funding::start(tx, &send, command, input.evidence).await? {
                    // 现金门不放行（账实缺口或该 UID 有未收口外发）：与拍卖通用现金门同口径，本事务只提交
                    // 现金门写入的账户冻结；不写起写事实、不写账本回执（同命令补足解冻后可重试）、不发许可，
                    // 以确定的“不可发”拒绝回答（不变量 4、6）。
                    tracing::warn!(
                        code = "dispatch_cash_gate_refused",
                        "寄售现金腿未过现金门，不起写"
                    );
                    return Ok(Step::Rejected(Rejection::ActionNotAllowed));
                }
            }
            write_first_start(tx, &ledger, command, &round, &delivery, attempt_id).await?;
            insert_success(
                tx.connection(),
                &ledger,
                "write_started",
                json!({
                    "delivery_id": delivery_id,
                    "attempt_id": attempt_id,
                    "fence": command.attempt.fence,
                }),
            )
            .await?;
            // 起写与订单阶段事件、权利刷新同事务（不变量 3）：起写回滚则事件零行。
            let now = database_now(tx.connection()).await?;
            record_order_stage_change_in_tx(tx.connection(), round.key.contract_id, now).await?;
            let receipt =
                start_receipt(command.command_id, &round, attempt_id, command.attempt.fence);
            Ok(Step::Done(WriteStartOutcome::FirstCommitted(receipt)))
        }
    }
}

/// 寄售轮的收件人授权：`FOR SHARE` 锁受益用户 → 锁投递行（读冻结收件名）→ 锁履约行 → 判定。
///
/// 保护不变量 2、4、6：锁序照 D9 全局顺序 `users → obligations → allocations → deliveries →
/// fulfillments`，不与结算事务成环；判定只在本事务内有效，事务提交到外写之间的改名由持久起写的
/// 冻结身份与人工核对承担。返回 `None` 表示授权；保留权利映射为 `EvidenceUnproven`，义务不属寄售
/// 合同或投递缺失按来源不符失败关闭。起写、登记尝试与首轮衔接共用。
pub(super) async fn authorize_round_recipient(
    connection: &mut PgConnection,
    obligation_id: Uuid,
    delivery_id: Uuid,
) -> Result<Option<Rejection>, WriteError> {
    lock_send_beneficiary(&mut *connection, obligation_id).await?;
    let recipient: Option<String> =
        sqlx::query_scalar("SELECT recipient FROM public.deliveries WHERE id = $1 FOR UPDATE")
            .bind(delivery_id)
            .fetch_optional(&mut *connection)
            .await?;
    let Some(recipient) = recipient else {
        return Ok(Some(Rejection::SourceMismatch));
    };
    sqlx::query(
        "SELECT 1 FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1 FOR UPDATE",
    )
    .bind(obligation_id)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(match authorize_consignment_send(connection, obligation_id, &recipient).await? {
        EntitlementDecision::Authorized => None,
        EntitlementDecision::Retain(_reason) => Some(Rejection::EvidenceUnproven),
        EntitlementDecision::NotConsignment => Some(Rejection::SourceMismatch),
    })
}

/// 回执里 `decision_id` 载轮 ID、`work_id` 载投递 ID（端口约定）。
fn start_receipt(
    command_id: CommandId,
    round: &StoredRound,
    attempt_id: Uuid,
    fence: u64,
) -> StartReceipt {
    StartReceipt::started(command_id, round.round_id, round.delivery_id, attempt_id, fence)
}

/// 由已存的成功回执正文还原起写回执；回放永远是“已提交”。
async fn receipt_from_body(
    command_id: CommandId,
    body: &Value,
    connection: &mut PgConnection,
) -> Result<StartReceipt, WriteError> {
    let field = |name: &str| -> Result<Uuid, WriteError> {
        body.get(name)
            .and_then(Value::as_str)
            .and_then(|text| Uuid::parse_str(text).ok())
            .ok_or_else(|| WriteError::invariant("consignment_dispatch_receipt_body"))
    };
    let delivery_id = field("delivery_id")?;
    let attempt_id = field("attempt_id")?;
    let fence = body
        .get("fence")
        .and_then(Value::as_u64)
        .ok_or_else(|| WriteError::invariant("consignment_dispatch_receipt_body"))?;
    let round = load_by_delivery(connection, delivery_id)
        .await?
        .ok_or_else(|| WriteError::invariant("consignment_dispatch_round_missing"))?;
    Ok(start_receipt(command_id, &round, attempt_id, fence))
}

/// 首次起写的持久写：投递转 sending、冻结 Send 身份、登记尝试、原件腿推进 position、审计。
async fn write_first_start(
    tx: &mut DepositTransaction<'_, '_>,
    ledger: &LedgerCommand,
    command: &MarkWriteStarted,
    round: &StoredRound,
    delivery: &DeliveryRow,
    attempt_id: Uuid,
) -> Result<(), WriteError> {
    let fence = i64::try_from(command.attempt.fence)?;
    let delivery_id = command.attempt.delivery_id;
    let changed = sqlx::query(
        "UPDATE public.deliveries
         SET status = 'sending', sending_at = statement_timestamp(),
             updated_at = statement_timestamp()
         WHERE id = $1 AND status = 'claimed' AND lease_token = $2 AND fencing_version = $3",
    )
    .bind(delivery_id)
    .bind(command.claim_token)
    .bind(fence)
    .execute(tx.connection())
    .await?;
    if changed.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_dispatch_sending_cas"));
    }
    let equipment_ids = delivery.payload.get("equipment_ids").cloned().unwrap_or_else(|| json!([]));
    let expectation = json!({
        "world_id": round.world_id,
        "recipient": delivery.recipient,
        "subject": round.subject,
        "body_digest": round.body_digest,
        "credits": round.credits,
        "cod": round.cod,
        "equipment_ids": equipment_ids,
        // 与拍卖冻结的 `DeliverySendCredentialBinding` 同形。
        "credential": command.credential.as_ref().map_or(Value::Null, |binding| json!({
            "credential_id": binding.credential_id,
            "credential_version": binding.credential_version,
            "runtime_generation": binding.runtime_generation,
        })),
    });
    // 与 `delivery_send_attempts` 的冻结写同形：命令、摘要与 sending_at 取自刚转态的投递行。
    sqlx::query(
        "INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         SELECT id, $2, command_id, request_digest, $3, sending_at, $4
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .bind(fence)
    .bind(&round.world_id)
    .bind(&expectation)
    .execute(tx.connection())
    .await?;
    let kind: String = sqlx::query_scalar(
        "SELECT obligation_kind FROM public.settlement_obligations WHERE id = $1",
    )
    .bind(round.key.obligation_id)
    .fetch_one(tx.connection())
    .await?;
    let leg = leg_for(&kind).ok_or_else(|| WriteError::invariant("consignment_dispatch_leg"))?;
    sqlx::query(
        "INSERT INTO public.consignment_dispatch_attempts
            (delivery_id, fence, attempt_id, contract_id, obligation_id, round, leg, started_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, statement_timestamp())",
    )
    .bind(delivery_id)
    .bind(fence)
    .bind(attempt_id)
    .bind(round.key.contract_id)
    .bind(round.key.obligation_id)
    .bind(i32::try_from(round.key.round)?)
    .bind(leg)
    .execute(tx.connection())
    .await?;
    if leg == "original" {
        advance_position_in_transit(tx.connection(), round.key.contract_id, ledger.command_id)
            .await?;
    }
    let occurred_at =
        crate::database::consignment::deposit::primitives::database_now(tx.connection()).await?;
    append_delivery_audit(
        tx.tracked(),
        DeliveryAudit {
            occurred_at,
            actor_type: "worker",
            actor_id: "consignment-dispatch",
            action: START_SENDING,
            delivery_id,
            request_id: Some(&ledger.command_id.to_string()),
            source: "worker",
            metadata: worker_state_metadata(
                "sending",
                fence,
                u32::try_from(delivery.attempt_count)?,
                None,
            ),
        },
    )
    .await?;
    Ok(())
}

/// 原件腿首次起写：Sale 的 position `committed → in_transit`，撤回件（PIWX）的 `in_custody → in_transit`，
/// 并追加同版本代管事件。
///
/// 保护不变量 3、6、8：状态推进与不可变事件同事务，触发器再核同事务存在该合同原件腿的尝试与合同种类。
/// 已在途（后续原件腿）或核对中的 position 不改；其他前态（如 Sale 仍 `in_custody`、已交付、已灭失）
/// 说明合同与实物不符，失败关闭，整笔起写回滚、不发 POST 许可。
async fn advance_position_in_transit(
    connection: &mut PgConnection,
    contract_id: Uuid,
    command_id: Uuid,
) -> Result<(), WriteError> {
    let position: Option<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT position.id, position.listing_id, position.state, contract.kind
         FROM public.consignment_contracts AS contract
         JOIN public.consignment_custody_positions AS position ON position.id = contract.position_id
         WHERE contract.contract_id = $1 FOR UPDATE OF position",
    )
    .bind(contract_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((position_id, listing_id, state, kind)) = position else {
        return Err(WriteError::invariant("consignment_dispatch_position_missing"));
    };
    let from_state = match (kind.as_str(), state.as_str()) {
        ("sale", "committed") | ("withdrawal", "in_custody") => state,
        (_, "in_transit" | "reconcile") => return Ok(()),
        _ => return Err(WriteError::invariant("consignment_dispatch_position_state")),
    };
    let version: i64 = sqlx::query_scalar(
        "UPDATE public.consignment_custody_positions
         SET state = 'in_transit', version = version + 1, updated_at = statement_timestamp()
         WHERE id = $1 AND state = $2 RETURNING version",
    )
    .bind(position_id)
    .bind(&from_state)
    .fetch_one(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         SELECT $1, $2, $3, 'in_transit', $5, 'in_transit', NULL, $4, state_version,
                statement_timestamp()
         FROM public.consignment_listings WHERE id = $4",
    )
    .bind(position_id)
    .bind(version)
    .bind(command_id)
    .bind(listing_id)
    .bind(&from_state)
    .execute(&mut *connection)
    .await?;
    Ok(())
}
