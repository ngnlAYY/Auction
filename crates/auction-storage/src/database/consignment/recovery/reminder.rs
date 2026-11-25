//! 付款提醒写端：到期且已证寄出、仍未付才排一封通用投递；每合同一次；派前再验，可能已派零补发（7A-2）。
//!
//! 保护不变量 3、4、5：决定、投递行与成功回执同事务提交，`consignment_payment_reminders.contract_id`
//! 主键保证每合同只有一次决定；是否到期由数据库时钟判定，内存定时器只负责唤醒；
//! 派出前的复验只读、只给出结论，提醒可能已派出（投递已离开 `pending`）时一律零补发，结果未知不自动重发。
//! 逾期本身不处罚，本写端也不改任何合同、挂单或资金行。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, RecoveryOperation, RemindContract, ReminderOutcome,
    ReminderReceipt, payment_reminder_delivery_command, payment_reminder_delivery_id,
};
use auction_application::notification_metrics::NotificationRecorder;
use auction_domain::CommandId;
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_domain::consignment::recovery::{
    ReminderDecision, ReminderFacts, ReminderStage, decide_reminder,
};
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::ledger::{
    Arbitration, LedgerCommand, Rejection, Step, Target, arbitrate, insert_success,
};
use crate::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::mail::delivery::{
    MailDeliveryTransactionError, enqueue_in_transaction_with_id,
};
use crate::database::mail::originals::invariant_decoding;

/// 合同上的提醒事实；读不出的事实取失败关闭的值。
struct ContractFacts {
    buyer_identity: String,
    world: String,
    due: bool,
    deadline_at: Timestamp,
    shipment_proven: bool,
    paid: bool,
    cancelled: bool,
}

async fn contract_facts(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Option<ContractFacts>, WriteError> {
    let row = sqlx::query(
        "SELECT sale.frozen_buyer_identity, contract.world_id, sale.payment_deadline_at,
                sale.payment_deadline_at <= clock_timestamp() AS due,
                EXISTS (SELECT 1 FROM public.consignment_dispatch_rounds AS round
                        JOIN public.delivery_sent_bindings AS sent
                          ON sent.delivery_id = round.delivery_id
                        WHERE round.contract_id = sale.contract_id) AS shipment_proven,
                EXISTS (SELECT 1 FROM public.consignment_funded_sales AS funded
                        WHERE funded.contract_id = sale.contract_id) AS paid,
                EXISTS (SELECT 1 FROM public.consignment_return_dispositions AS disposition
                        WHERE disposition.contract_id = sale.contract_id
                          AND disposition.buyer_right IN (
                              'close_at_limit', 'terminate_unpaid', 'convert_to_refund',
                              'terminate_carrier_leg_only', 'close_old_season')) AS cancelled
         FROM public.consignment_sales AS sale
         JOIN public.consignment_contracts AS contract ON contract.contract_id = sale.contract_id
         WHERE sale.contract_id = $1",
    )
    .bind(contract_id)
    .fetch_optional(connection)
    .await?;
    row.map(|row| {
        Ok(ContractFacts {
            buyer_identity: row.try_get("frozen_buyer_identity")?,
            world: row.try_get("world_id")?,
            due: row.try_get("due")?,
            deadline_at: row.try_get("payment_deadline_at")?,
            shipment_proven: row.try_get("shipment_proven")?,
            paid: row.try_get("paid")?,
            cancelled: row.try_get("cancelled")?,
        })
    })
    .transpose()
}

/// 预读：买家与规范实物，只用来决定取哪些会话锁，不具授权含义。
async fn pre_read(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Option<(Uuid, ListingEquipmentIdentity)>, WriteError> {
    let row = sqlx::query(
        "SELECT sale.buyer_user_id, contract.world_id, contract.canonical_eid
         FROM public.consignment_sales AS sale
         JOIN public.consignment_contracts AS contract ON contract.contract_id = sale.contract_id
         WHERE sale.contract_id = $1",
    )
    .bind(contract_id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let equipment = ListingEquipmentIdentity::new(
        &row.try_get::<String, _>("world_id")?,
        &row.try_get::<String, _>("canonical_eid")?,
    )
    .map_err(invariant_decoding("consignment_reminder_equipment"))?;
    Ok(Some((row.try_get("buyer_user_id")?, equipment)))
}

/// 付款提醒端口实现：预读锁集合后在一个顶层 Serializable 事务内完成。
pub(super) async fn remind(
    store: &DatabaseStore,
    command: RemindContract,
) -> Result<ReminderReceipt, ConsignmentError> {
    let mut pre =
        store.pool().acquire().await.map_err(|error| WriteError::from(error).into_application())?;
    let plan =
        pre_read(&mut pre, command.contract_id).await.map_err(WriteError::into_application)?;
    drop(pre);
    let Some((buyer, equipment)) = plan else {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch));
    };
    let (sellers, equipment) = (vec![buyer], vec![equipment]);
    let keys = LockKeys {
        command_id: command.command_id.as_uuid(),
        sellers: &sellers,
        equipment: &equipment,
    };
    let (locked_sellers, locked_equipment) = (sellers.clone(), equipment.clone());
    let notifications = std::sync::Arc::clone(&store.notification_metrics);
    let step = with_deposit_transaction(store, keys, &[], &command, |command, tx| {
        let (sellers, equipment) = (locked_sellers.clone(), locked_equipment.clone());
        let notifications = std::sync::Arc::clone(&notifications);
        Box::pin(async move {
            tx.ensure_lock_set(&sellers, &equipment, &[])?;
            remind_in_tx(tx, command, notifications.as_ref()).await
        })
    })
    .await
    .map_err(WriteError::into_application)?;
    step.into_result()
}

fn receipt(command: &RemindContract, outcome: ReminderOutcome, replayed: bool) -> ReminderReceipt {
    ReminderReceipt {
        command_id: CommandId::from_uuid(command.command_id.as_uuid()),
        outcome,
        replayed,
    }
}

fn decision_code(decision: ReminderDecision) -> Option<&'static str> {
    match decision {
        ReminderDecision::VoidPaid => Some("void_paid"),
        ReminderDecision::VoidCancelled => Some("void_cancelled"),
        ReminderDecision::VoidRound => Some("void_round"),
        _ => None,
    }
}

fn decision_of(code: &str) -> Option<ReminderDecision> {
    match code {
        "void_paid" => Some(ReminderDecision::VoidPaid),
        "void_cancelled" => Some(ReminderDecision::VoidCancelled),
        "void_round" => Some(ReminderDecision::VoidRound),
        _ => None,
    }
}

async fn remind_in_tx(
    tx: &mut DepositTransaction<'_, '_>,
    command: &RemindContract,
    notifications: &dyn NotificationRecorder,
) -> Result<Step<ReminderReceipt>, WriteError> {
    let ledger = LedgerCommand {
        command_id: command.command_id.as_uuid(),
        operation: RecoveryOperation::Remind,
        target: Target::Contract(command.contract_id),
        digest: &command.request_hash,
    };
    match arbitrate(tx.connection(), &ledger).await? {
        Step::Rejected(rejection) => return Ok(Step::Rejected(rejection)),
        Step::Done(Arbitration::Replay(body)) => {
            return replay_receipt(command, &body).map(Step::Done);
        }
        Step::Done(Arbitration::First) => {}
    }
    let Some(contract) = contract_facts(tx.connection(), command.contract_id).await? else {
        return Ok(Step::Rejected(Rejection::SourceMismatch));
    };
    let already_queued: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_payment_reminders WHERE contract_id = $1)",
    )
    .bind(command.contract_id)
    .fetch_one(tx.connection())
    .await?;
    let decision = decide_reminder(&ReminderFacts {
        stage: ReminderStage::Scheduling,
        due: contract.due,
        shipment_proven: contract.shipment_proven,
        already_queued,
        paid: Some(contract.paid),
        cancelled: Some(contract.cancelled),
        round_current: true,
        dispatch_possibly_sent: false,
    });
    let now = database_now(tx.connection()).await?;
    match decision {
        // 尚不需要、已建立过、事实未知：没有决定可作，不写任何行、不占命令 ID。
        ReminderDecision::NotDue | ReminderDecision::Withhold | ReminderDecision::NoResend => {
            Ok(Step::Done(receipt(command, ReminderOutcome::NotDue, false)))
        }
        ReminderDecision::Queue => {
            let delivery_id = queue_delivery(tx, command, &contract, notifications).await?;
            insert_reminder(tx.connection(), command, &contract, "queued", Some(delivery_id), now)
                .await?;
            insert_success(
                tx.connection(),
                &ledger,
                "reminder_queued",
                json!({
                    "contract_id": command.contract_id.to_string(),
                    "delivery_id": delivery_id.to_string(),
                }),
            )
            .await?;
            Ok(Step::Done(receipt(command, ReminderOutcome::Queued { delivery_id }, false)))
        }
        voided => {
            let code = decision_code(voided)
                .ok_or_else(|| WriteError::invariant("consignment_reminder_decision"))?;
            insert_reminder(tx.connection(), command, &contract, code, None, now).await?;
            insert_success(
                tx.connection(),
                &ledger,
                "reminder_voided",
                json!({
                    "contract_id": command.contract_id.to_string(),
                    "decision": code,
                }),
            )
            .await?;
            Ok(Step::Done(receipt(command, ReminderOutcome::Voided(voided), false)))
        }
    }
}

/// 重放：账本回执里的结果原样返回，不重算。
fn replay_receipt(command: &RemindContract, body: &Value) -> Result<ReminderReceipt, WriteError> {
    let outcome = match body.get("disposition").and_then(Value::as_str) {
        Some("reminder_queued") => ReminderOutcome::Queued {
            delivery_id: body
                .get("delivery_id")
                .and_then(Value::as_str)
                .and_then(|text| Uuid::parse_str(text).ok())
                .ok_or_else(|| WriteError::invariant("consignment_reminder_replay"))?,
        },
        Some("reminder_voided") => ReminderOutcome::Voided(
            body.get("decision")
                .and_then(Value::as_str)
                .and_then(decision_of)
                .ok_or_else(|| WriteError::invariant("consignment_reminder_replay"))?,
        ),
        _ => return Err(WriteError::invariant("consignment_reminder_replay")),
    };
    Ok(receipt(command, outcome, true))
}

/// 通用投递：经正规入队原语（`MailDeliveryCommand`）写入来源 `consignment_notice` 的 Send，
/// 模板与来源绑定、载荷形状与通知计量都由投递层校验；业务键随合同确定，同合同重放得到同一投递。
async fn queue_delivery(
    tx: &mut DepositTransaction<'_, '_>,
    command: &RemindContract,
    contract: &ContractFacts,
    notifications: &dyn NotificationRecorder,
) -> Result<Uuid, WriteError> {
    let delivery = payment_reminder_delivery_command(
        command.contract_id,
        &contract.world,
        &contract.buyer_identity,
    )
    .map_err(|error| {
        tracing::warn!(%error, "付款提醒的投递命令无法生成");
        WriteError::invariant("consignment_reminder_delivery_command")
    })?;
    let delivery_id = payment_reminder_delivery_id(command.contract_id);
    let stored = enqueue_in_transaction_with_id(
        tx.tracked(),
        &delivery,
        None,
        Some(notifications),
        delivery_id,
    )
    .await
    .map_err(|error| match error {
        MailDeliveryTransactionError::Sqlx(error) => WriteError::from(error),
        MailDeliveryTransactionError::Rejected(_) => {
            WriteError::invariant("consignment_reminder_delivery_rejected")
        }
    })?;
    if stored.delivery_id != delivery_id {
        return Err(WriteError::invariant("consignment_reminder_delivery_id"));
    }
    Ok(delivery_id)
}

async fn insert_reminder(
    connection: &mut PgConnection,
    command: &RemindContract,
    contract: &ContractFacts,
    decision: &str,
    delivery_id: Option<Uuid>,
    now: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_payment_reminders
            (contract_id, decision, due_at, decided_at, delivery_id, command_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(command.contract_id)
    .bind(decision)
    .bind(contract.deadline_at)
    .bind(now)
    .bind(delivery_id)
    .bind(command.command_id.as_uuid())
    .execute(connection)
    .await?;
    Ok(())
}

#[cfg(feature = "test-db")]
impl DatabaseStore {
    /// PG 测试入口：派出前再验（只读）；生产只经投递层的 sending 围栏调用 `pre_dispatch_in`。
    pub async fn test_reminder_pre_dispatch(
        &self,
        contract_id: Uuid,
    ) -> Result<ReminderDecision, ConsignmentError> {
        let mut connection = self
            .pool()
            .acquire()
            .await
            .map_err(|error| WriteError::from(error).into_application())?;
        pre_dispatch_in(&mut connection, contract_id).await.map_err(WriteError::into_application)
    }
}

/// 派出前再验的事务内实现：投递层的 sending 围栏与端口的只读复验共用。
pub(crate) async fn pre_dispatch_in(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<ReminderDecision, WriteError> {
    let reminder = sqlx::query(
        "SELECT reminder.decision, reminder.decided_at, delivery.status, delivery.sending_at
         FROM public.consignment_payment_reminders AS reminder
         LEFT JOIN public.deliveries AS delivery ON delivery.id = reminder.delivery_id
         WHERE reminder.contract_id = $1",
    )
    .bind(contract_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(reminder) = reminder else { return Ok(ReminderDecision::NotDue) };
    if reminder.try_get::<&str, _>("decision")? != "queued" {
        return Ok(ReminderDecision::NotDue);
    }
    let decided_at: Timestamp = reminder.try_get("decided_at")?;
    let status: Option<String> = reminder.try_get("status")?;
    let sending_at: Option<Timestamp> = reminder.try_get("sending_at")?;
    let Some(contract) = contract_facts(connection, contract_id).await? else {
        return Err(WriteError::invariant("consignment_reminder_contract"));
    };
    // 提醒排队之后又有新一轮寄出：旧提醒对应的轮已变。
    let newer_round: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_dispatch_rounds
                        WHERE contract_id = $1 AND created_at > $2)",
    )
    .bind(contract_id)
    .bind(decided_at)
    .fetch_one(connection)
    .await?;
    Ok(decide_reminder(&ReminderFacts {
        stage: ReminderStage::PreDispatch,
        due: true,
        shipment_proven: true,
        already_queued: true,
        paid: Some(contract.paid),
        cancelled: Some(contract.cancelled),
        round_current: !newer_round,
        // 只有从未进入 sending 的 pending / claimed 才确定未派出；其余（含结果未知）一律按可能已派处理。
        dispatch_possibly_sent: !matches!(status.as_deref(), Some("pending" | "claimed"))
            || sending_at.is_some(),
    }))
}

/// 已到期、已证寄出且尚无提醒决定的合同（仅供 Worker 轮询，没有授权含义）。
/// 到期未提醒的已售合同：付款期限已到（数据库时钟）、还没有提醒决定、且至少一轮已证寄出。
/// `due_reminders` 与寄售存量探针共用这一判据，两处逐字一致（不变量 5：待办只来自持久事实）。
macro_rules! due_reminder_source {
    () => {
        "FROM public.consignment_sales AS sale
         WHERE sale.payment_deadline_at <= clock_timestamp()
           AND NOT EXISTS (
               SELECT 1 FROM public.consignment_payment_reminders AS reminder
               WHERE reminder.contract_id = sale.contract_id)
           AND EXISTS (
               SELECT 1 FROM public.consignment_dispatch_rounds AS round
               JOIN public.delivery_sent_bindings AS sent ON sent.delivery_id = round.delivery_id
               WHERE round.contract_id = sale.contract_id)"
    };
}
pub(crate) use due_reminder_source;

pub(super) async fn due_reminders(
    store: &DatabaseStore,
    limit: u32,
) -> Result<Vec<Uuid>, ConsignmentError> {
    sqlx::query_scalar(concat!(
        "SELECT sale.contract_id ",
        due_reminder_source!(),
        " ORDER BY sale.payment_deadline_at, sale.contract_id
         LIMIT $1",
    ))
    .bind(i64::from(limit))
    .fetch_all(store.pool())
    .await
    .map_err(|error| WriteError::from(error).into_application())
}
