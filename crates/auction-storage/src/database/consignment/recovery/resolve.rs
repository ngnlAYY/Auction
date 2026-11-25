//! 退回处置写端：对一次精确 Take 消费，按 D9 §5.1 矩阵作出处置并落库（7A-2）。
//!
//! 保护不变量 1、3、6、8、10：同一消费只有一份处置（`UNIQUE(consumption_id)` 加账本重放）；矩阵的决定、
//! 决策时冻结的自动重列开关与配置摘要同事务追加，之后改配置不影响已作的决定；事实互相矛盾、
//! 可交付性不明或相反付款可能已发时领域给出“留案”，零资金动作、禁止再售，而不是猜测。
//!
//! 处置的应用效果（封闭原买方交割、挂单转下架）在同一事务里由 `resolve_effects` 写入：
//! 未付收回处置是这两项的 owner；退款与本金的回补已由 6D5 的消费事务完成。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, PendingResolution, RecoveryOperation,
    ResolveReceipt, ResolveReturn,
};
use auction_domain::consignment::recovery::{
    BuyerRightEffect, DeliveryClosure, ListingEffect, PrincipalEffect, RecoveryDomainError,
    RefundEffect, RelistEligibility, ReturnDispositionDecision, ReturnDispositionFacts,
    ReturnedLeg, decide_return_disposition,
};
use auction_domain::consignment::returns::ReturnLeg;
use auction_domain::consignment::{ListingEquipmentIdentity, parse_settlement_world};
use auction_domain::{CommandId, SettlementWorld};
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::ledger::{Arbitration, LedgerCommand, Rejection, Step, arbitrate, insert_success};
use super::resolve_effects::{apply, plan};
use crate::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::returns::funding_error;
use crate::database::mail::originals::invariant_decoding;
use crate::database::settlement::funding::lock_funding_worlds;

const BUYER_RIGHT: [(BuyerRightEffect, &str); 8] = [
    (BuyerRightEffect::Retain, "retain"),
    (BuyerRightEffect::CloseAtLimit, "close_at_limit"),
    (BuyerRightEffect::TerminateUnpaid, "terminate_unpaid"),
    (BuyerRightEffect::ConvertToRefund, "convert_to_refund"),
    (BuyerRightEffect::TerminateCarrierLegOnly, "terminate_carrier_leg_only"),
    (BuyerRightEffect::CloseOldSeason, "close_old_season"),
    (BuyerRightEffect::NotApplicable, "not_applicable"),
    (BuyerRightEffect::HoldForCase, "hold_for_case"),
];
const REFUND: [(RefundEffect, &str); 3] = [
    (RefundEffect::None, "none"),
    (RefundEffect::RefundP, "refund_p"),
    (RefundEffect::RestoreRefundPurpose, "restore_refund_purpose"),
];
const PRINCIPAL: [(PrincipalEffect, &str); 3] = [
    (PrincipalEffect::None, "none"),
    (PrincipalEffect::RestoreOriginalCarrier, "restore_original_carrier"),
    (PrincipalEffect::WriteOffOnly, "write_off_only"),
];
const LISTING: [(ListingEffect, &str); 3] = [
    (ListingEffect::Unchanged, "unchanged"),
    (ListingEffect::HeldUnpaidRecovered, "held_unpaid_recovered"),
    (ListingEffect::HeldPaidReturned, "held_paid_returned"),
];
const RELIST: [(RelistEligibility, &str); 3] = [
    (RelistEligibility::Forbidden, "forbidden"),
    (RelistEligibility::ManualOnly, "manual_only"),
    (RelistEligibility::ManualOrAuto, "manual_or_auto"),
];

fn code<T: Copy + PartialEq>(
    table: &[(T, &'static str)],
    value: T,
) -> Result<&'static str, WriteError> {
    table
        .iter()
        .find(|(candidate, _)| *candidate == value)
        .map(|(_, text)| *text)
        .ok_or_else(|| WriteError::invariant("consignment_disposition_code"))
}

fn parse<T: Copy>(table: &[(T, &'static str)], text: &str) -> Result<T, WriteError> {
    table
        .iter()
        .find(|(_, candidate)| *candidate == text)
        .map(|(value, _)| *value)
        .ok_or_else(|| WriteError::invariant("consignment_disposition_decode"))
}

/// 预读：消费的买家、卖家与规范实物，只用来决定取哪些会话锁，不具授权含义。
struct LockPlan {
    sellers: Vec<Uuid>,
    equipment: Vec<ListingEquipmentIdentity>,
}

async fn pre_read(
    connection: &mut PgConnection,
    consumption_id: Uuid,
    intake_id: Uuid,
) -> Result<Option<LockPlan>, WriteError> {
    let row = sqlx::query(
        "SELECT contract.world_id, contract.canonical_eid, contract.seller_user_id,
                sale.buyer_user_id
         FROM public.consignment_return_consumptions AS consumption
         JOIN public.consignment_contracts AS contract
           ON contract.contract_id = consumption.contract_id
         LEFT JOIN public.consignment_sales AS sale ON sale.contract_id = contract.contract_id
         WHERE consumption.id = $1 AND consumption.intake_id = $2",
    )
    .bind(consumption_id)
    .bind(intake_id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let equipment = ListingEquipmentIdentity::new(
        &row.try_get::<String, _>("world_id")?,
        &row.try_get::<String, _>("canonical_eid")?,
    )
    .map_err(invariant_decoding("consignment_disposition_equipment"))?;
    let mut sellers = vec![row.try_get::<Uuid, _>("seller_user_id")?];
    sellers.extend(row.try_get::<Option<Uuid>, _>("buyer_user_id")?);
    Ok(Some(LockPlan { sellers, equipment: vec![equipment] }))
}

/// 矩阵判定函数；生产恒为 [`decide_return_disposition`]，PG 测试经 `test-db` 入口换入固定决定，
/// 以便在不构造多轮退件的前提下端到端验证决定的应用效果。
type Decider =
    fn(&ReturnDispositionFacts) -> Result<ReturnDispositionDecision, RecoveryDomainError>;

/// 退回处置端口实现：预读锁集合后在一个顶层 Serializable 事务内完成。
pub(super) async fn resolve_return(
    store: &DatabaseStore,
    command: ResolveReturn,
) -> Result<ResolveReceipt, ConsignmentError> {
    resolve_return_with(store, command, decide_return_disposition).await
}

async fn resolve_return_with(
    store: &DatabaseStore,
    command: ResolveReturn,
    decide: Decider,
) -> Result<ResolveReceipt, ConsignmentError> {
    let mut pre =
        store.pool().acquire().await.map_err(|error| WriteError::from(error).into_application())?;
    let plan = pre_read(&mut pre, command.consumption_id, command.intake_id)
        .await
        .map_err(WriteError::into_application)?;
    drop(pre);
    let Some(plan) = plan else {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch));
    };
    let keys = LockKeys {
        command_id: command.command_id.as_uuid(),
        sellers: &plan.sellers,
        equipment: &plan.equipment,
    };
    let (sellers, equipment) = (plan.sellers.clone(), plan.equipment.clone());
    let step = with_deposit_transaction(store, keys, &[], &command, |command, tx| {
        let (sellers, equipment) = (sellers.clone(), equipment.clone());
        Box::pin(async move {
            tx.ensure_lock_set(&sellers, &equipment, &[])?;
            resolve_in_tx(tx, command, decide).await
        })
    })
    .await
    .map_err(WriteError::into_application)?;
    step.into_result()
}

async fn resolve_in_tx(
    tx: &mut DepositTransaction<'_, '_>,
    command: &ResolveReturn,
    decide: Decider,
) -> Result<Step<ResolveReceipt>, WriteError> {
    let ledger = LedgerCommand {
        command_id: command.command_id.as_uuid(),
        operation: RecoveryOperation::ResolveReturn,
        target: super::ledger::Target::Intake(command.intake_id),
        digest: &command.request_hash,
    };
    match arbitrate(tx.connection(), &ledger).await? {
        Step::Rejected(rejection) => return Ok(Step::Rejected(rejection)),
        Step::Done(Arbitration::Replay(body)) => {
            return replay_receipt(tx.connection(), command, &body).await.map(Step::Done);
        }
        Step::Done(Arbitration::First) => {}
    }
    // 锁序：funding worlds 先于合同、投递等业务行（效果里的 cancel_unpaid 随后取 UID 账户锁）。
    lock_funding_worlds(tx.tracked()).await.map_err(funding_error)?;
    let Some(loaded) = load(tx.connection(), command).await? else {
        return Ok(Step::Rejected(Rejection::SourceMismatch));
    };
    // 同一消费已有另一份处置（异命令 ID）：不另造第二份，也不改写原处置（不变量 1、8）。
    if disposition_of(tx.connection(), command.consumption_id).await?.is_some() {
        return Ok(Step::Rejected(Rejection::DispositionConflict));
    }
    let decision = match decide(&loaded.facts) {
        Ok(decision) => decision,
        Err(error) => {
            tracing::warn!(%error, "退回处置事实不成立，失败关闭");
            return Ok(Step::Rejected(Rejection::EvidenceConflict));
        }
    };
    // 效果的前置事实先全部核对（零写入）：不成立时是确定拒绝，不留下半份处置。
    let Some(effects) =
        plan(tx.connection(), command, loaded.contract_id, loaded.listing_id, decision).await?
    else {
        return Ok(Step::Rejected(Rejection::EvidenceConflict));
    };
    let disposition_id = Uuid::new_v4();
    let now = database_now(tx.connection()).await?;
    insert_disposition(tx.connection(), command, &loaded, decision, disposition_id, now).await?;
    let listing_change = effects.listing_change();
    let mut body = json!({
        "contract_id": loaded.contract_id.to_string(),
        // 合同不可变，版本恒为 1（与消费回执一致）。
        "contract_version": 1,
        "consumption_id": command.consumption_id.to_string(),
        "disposition_id": disposition_id.to_string(),
    });
    if let Some(change) = listing_change {
        // 挂单版本事件核对读回执里的挂单变更（与其他改挂单的命令同形）。
        body["listing_changes"] = json!([{
            "listing_id": change.listing_id.to_string(),
            "listing_version": change.listing_version,
            "state": "held_off_market",
            "hold_reason": "unpaid_recovered",
        }]);
    }
    // 回执先于挂单事件入账（事件对命令的外键不可延迟）；随后写效果。
    insert_success(tx.connection(), &ledger, "disposition_recorded", body).await?;
    apply(tx, command.command_id.as_uuid(), loaded.contract_id, disposition_id, effects, now)
        .await?;
    Ok(Step::Done(ResolveReceipt {
        command_id: command.command_id,
        contract_id: loaded.contract_id,
        consumption_id: command.consumption_id,
        disposition_id,
        decision,
        replayed: false,
    }))
}

/// 锁内读出的事实与合同身份。
struct Loaded {
    contract_id: Uuid,
    listing_id: Uuid,
    facts: ReturnDispositionFacts,
    leg: ReturnLeg,
}

/// 把锁内事实装配成矩阵输入。读不出的事实一律取失败关闭的值（留案），不推测。
async fn load(
    connection: &mut PgConnection,
    command: &ResolveReturn,
) -> Result<Option<Loaded>, WriteError> {
    let row = sqlx::query(
        "SELECT consumption.contract_id, consumption.leg, contract.kind AS contract_kind,
                contract.listing_id,
                contract.world_id, contract.season_window_id,
                sale.buyer_user_id, sale.cod_return_limit
         FROM public.consignment_return_consumptions AS consumption
         JOIN public.consignment_contracts AS contract
           ON contract.contract_id = consumption.contract_id
         LEFT JOIN public.consignment_sales AS sale ON sale.contract_id = contract.contract_id
         WHERE consumption.id = $1 AND consumption.intake_id = $2",
    )
    .bind(command.consumption_id)
    .bind(command.intake_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let contract_id: Uuid = row.try_get("contract_id")?;
    let leg = ReturnLeg::parse(&row.try_get::<String, _>("leg")?)
        .map_err(invariant_decoding("consignment_disposition_leg"))?;
    let world = parse_settlement_world(&row.try_get::<String, _>("world_id")?)
        .ok_or_else(|| WriteError::invariant("consignment_disposition_world"))?;
    let kind: String = row.try_get("contract_kind")?;
    let season_window_id: Option<Uuid> = row.try_get("season_window_id")?;
    let cod_return_limit: Option<i16> = row.try_get("cod_return_limit")?;
    let returned_leg = returned_leg(&kind, world, leg);
    let (closure, opposite_by_verdict, evidence_conflict) =
        verdict_facts(connection, command.intake_id, returned_leg).await?;
    let funded: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_funded_sales WHERE contract_id = $1)",
    )
    .bind(contract_id)
    .fetch_one(&mut *connection)
    .await?;
    let original_returns: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_return_consumptions
         WHERE contract_id = $1 AND leg = 'original'",
    )
    .bind(contract_id)
    .fetch_one(&mut *connection)
    .await?;
    let original_taken = original_exactly_taken(connection, contract_id).await?;
    let principal_taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_return_consumptions
                        WHERE contract_id = $1 AND leg = 'cash_principal')",
    )
    .bind(contract_id)
    .fetch_one(&mut *connection)
    .await?;
    let season_ended = season_ended(connection, season_window_id).await?;
    let unpaid_leg =
        matches!(returned_leg, ReturnedLeg::MainUnpaidOriginal | ReturnedLeg::IsekaiUnpaidCarrier);
    let facts = ReturnDispositionFacts {
        leg: returned_leg,
        world,
        closure,
        original_exactly_taken: original_taken,
        principal_exactly_taken: principal_taken,
        unpaid_returns_reached_limit: cod_return_limit
            .is_some_and(|limit| original_returns >= i64::from(limit)),
        // 未付腿已有付款（Sale 已资金化）即相反付款；已付交接的相反卖款是否封闭没有可读事实，
        // 保守视为未封闭（留案）。待 6B 提供卖款封闭读口后放宽。
        opposite_payment_open: opposite_by_verdict
            || (unpaid_leg && funded)
            || returned_leg == ReturnedLeg::IsekaiPaidHandoff,
        evidence_conflict,
        season_ended,
        // 原款是否在账没有可读事实；保守视为不在账，已付交接因此留案（不退 P）。
        original_payment_in_account: false,
    };
    Ok(Some(Loaded { contract_id, listing_id: row.try_get("listing_id")?, facts, leg }))
}

/// 原件“精确收回”只认 6D5 的消费事实：原件腿有一次消费，且引用精确 Take 的 `take_proven` 证明
/// （D9 §5.1）。不以 position 状态代替：isekai 未付载体的原物从未离库，没有消费事实时不能当作已证收回。
async fn original_exactly_taken(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.consignment_return_consumptions AS consumption
             JOIN public.mail_disposition_proofs AS proof ON proof.id = consumption.proof_id
             WHERE consumption.contract_id = $1 AND consumption.leg = 'original'
               AND proof.kind = 'exact_take' AND proof.conclusion = 'take_proven')",
    )
    .bind(contract_id)
    .fetch_one(connection)
    .await?)
}

#[cfg(feature = "test-db")]
impl DatabaseStore {
    /// PG 测试入口：以给定的判定函数（而不是 D9 §5.1 矩阵）作出处置，其余写序与生产一致。
    pub async fn test_resolve_return_with(
        &self,
        command: ResolveReturn,
        decide: fn(
            &ReturnDispositionFacts,
        ) -> Result<ReturnDispositionDecision, RecoveryDomainError>,
    ) -> Result<ResolveReceipt, ConsignmentError> {
        resolve_return_with(self, command, decide).await
    }

    /// PG 测试入口：原件是否已有精确收回的消费事实；与处置矩阵取数同一份实现，供 PG 用例直接断言。
    pub async fn test_original_exactly_taken(
        &self,
        contract_id: Uuid,
    ) -> Result<bool, ConsignmentError> {
        let mut connection = self
            .pool()
            .acquire()
            .await
            .map_err(|error| WriteError::from(error).into_application())?;
        original_exactly_taken(&mut connection, contract_id)
            .await
            .map_err(WriteError::into_application)
    }
}

fn returned_leg(kind: &str, world: SettlementWorld, leg: ReturnLeg) -> ReturnedLeg {
    if kind == "withdrawal" {
        return ReturnedLeg::WithdrawalReturn;
    }
    match leg {
        ReturnLeg::Original if world == SettlementWorld::Main => ReturnedLeg::MainUnpaidOriginal,
        ReturnLeg::Original => ReturnedLeg::IsekaiPaidHandoff,
        ReturnLeg::Carrier | ReturnLeg::CashPrincipal => ReturnedLeg::IsekaiUnpaidCarrier,
        ReturnLeg::CashRefund => ReturnedLeg::BuyerRefund,
        ReturnLeg::CashPayout => ReturnedLeg::SellerPayout,
    }
}

/// 6D3 的返回事实与最新裁决 -> (可交付性, 相反付款, 证据冲突)。
///
/// 没有返回事实：未付腿无法判定可交付性（留案），其余腿不涉及可交付性。未知裁决一律留案。
async fn verdict_facts(
    connection: &mut PgConnection,
    intake_id: Uuid,
    leg: ReturnedLeg,
) -> Result<(DeliveryClosure, bool, bool), WriteError> {
    let row = sqlx::query(
        "SELECT verdict.verdict, verdict.reason
         FROM public.consignment_return_facts AS fact
         JOIN LATERAL (
             SELECT candidate.verdict, candidate.reason
             FROM public.consignment_return_fact_verdicts AS candidate
             WHERE candidate.return_fact_id = fact.return_fact_id
             ORDER BY (candidate.verdict = 'qualified') DESC, candidate.evaluated_at DESC,
                      candidate.id DESC
             LIMIT 1
         ) AS verdict ON true
         WHERE fact.intake_id = $1",
    )
    .bind(intake_id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else {
        let unpaid =
            matches!(leg, ReturnedLeg::MainUnpaidOriginal | ReturnedLeg::IsekaiUnpaidCarrier);
        let closure =
            if unpaid { DeliveryClosure::Unclear } else { DeliveryClosure::StillDeliverable };
        return Ok((closure, false, false));
    };
    let verdict: String = row.try_get("verdict")?;
    let reason: Option<String> = row.try_get("reason")?;
    Ok(match (verdict.as_str(), reason.as_deref()) {
        ("qualified", _) => (DeliveryClosure::StillDeliverable, false, false),
        ("pending", _) => (DeliveryClosure::Unclear, false, false),
        ("not_counted", Some("after_delivery_closed")) => {
            (DeliveryClosure::ClosedBeforeRts, false, false)
        }
        ("not_counted", Some("paid_or_opposite")) => {
            (DeliveryClosure::StillDeliverable, true, false)
        }
        ("not_counted", Some("wrong_asset" | "duplicate")) => {
            (DeliveryClosure::StillDeliverable, false, true)
        }
        _ => (DeliveryClosure::Unclear, false, false),
    })
}

async fn season_ended(
    connection: &mut PgConnection,
    season_window_id: Option<Uuid>,
) -> Result<bool, WriteError> {
    let Some(window) = season_window_id else { return Ok(false) };
    let end: Option<Timestamp> =
        sqlx::query_scalar("SELECT end_utc FROM public.consignment_season_windows WHERE id = $1")
            .bind(window)
            .fetch_optional(&mut *connection)
            .await?;
    let now = database_now(connection).await?;
    Ok(end.is_some_and(|end| end <= now))
}

async fn insert_disposition(
    connection: &mut PgConnection,
    command: &ResolveReturn,
    loaded: &Loaded,
    decision: ReturnDispositionDecision,
    disposition_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_return_dispositions
            (disposition_id, consumption_id, contract_id, leg, buyer_right, refund, principal,
             listing_effect, relist, auto_relist_enabled, config_digest, command_id, decided_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(disposition_id)
    .bind(command.consumption_id)
    .bind(loaded.contract_id)
    .bind(loaded.leg.as_str())
    .bind(code(&BUYER_RIGHT, decision.buyer_right)?)
    .bind(code(&REFUND, decision.refund)?)
    .bind(code(&PRINCIPAL, decision.principal)?)
    .bind(code(&LISTING, decision.listing)?)
    .bind(code(&RELIST, decision.relist)?)
    .bind(command.auto_relist.enabled)
    .bind(command.auto_relist.config_digest.as_slice())
    .bind(command.command_id.as_uuid())
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// 已落库的处置行。
struct StoredDisposition {
    disposition_id: Uuid,
    contract_id: Uuid,
    decision: ReturnDispositionDecision,
}

async fn disposition_of(
    connection: &mut PgConnection,
    consumption_id: Uuid,
) -> Result<Option<StoredDisposition>, WriteError> {
    let row = sqlx::query(
        "SELECT disposition_id, contract_id, buyer_right, refund, principal, listing_effect, relist
         FROM public.consignment_return_dispositions WHERE consumption_id = $1",
    )
    .bind(consumption_id)
    .fetch_optional(connection)
    .await?;
    row.map(|row| {
        Ok(StoredDisposition {
            disposition_id: row.try_get("disposition_id")?,
            contract_id: row.try_get("contract_id")?,
            decision: ReturnDispositionDecision {
                buyer_right: parse(&BUYER_RIGHT, row.try_get::<&str, _>("buyer_right")?)?,
                refund: parse(&REFUND, row.try_get::<&str, _>("refund")?)?,
                principal: parse(&PRINCIPAL, row.try_get::<&str, _>("principal")?)?,
                listing: parse(&LISTING, row.try_get::<&str, _>("listing_effect")?)?,
                relist: parse(&RELIST, row.try_get::<&str, _>("relist")?)?,
            },
        })
    })
    .transpose()
}

/// 重放：账本回执指向的处置行原样返回，不重算、不依赖当前配置。
async fn replay_receipt(
    connection: &mut PgConnection,
    command: &ResolveReturn,
    body: &Value,
) -> Result<ResolveReceipt, WriteError> {
    let recorded = body
        .get("disposition_id")
        .and_then(Value::as_str)
        .and_then(|text| Uuid::parse_str(text).ok())
        .ok_or_else(|| WriteError::invariant("consignment_disposition_replay_body"))?;
    let stored = disposition_of(connection, command.consumption_id)
        .await?
        .filter(|stored| stored.disposition_id == recorded)
        .ok_or_else(|| WriteError::invariant("consignment_disposition_replay_row"))?;
    Ok(ResolveReceipt {
        command_id: CommandId::from_uuid(command.command_id.as_uuid()),
        contract_id: stored.contract_id,
        consumption_id: command.consumption_id,
        disposition_id: stored.disposition_id,
        decision: stored.decision,
        replayed: true,
    })
}

/// 尚无处置的消费，最早的在前（仅供 Worker 轮询，没有授权含义）。
/// 未处置的退件消费：消费行还没有任何处置事实。`pending_resolutions` 与寄售存量探针共用这一判据，
/// 两处逐字一致（不变量 5：待办只来自持久事实）。
macro_rules! pending_resolution_source {
    () => {
        "FROM public.consignment_return_consumptions AS consumption
         WHERE NOT EXISTS (
             SELECT 1 FROM public.consignment_return_dispositions AS disposition
             WHERE disposition.consumption_id = consumption.id)"
    };
}
pub(crate) use pending_resolution_source;

pub(super) async fn pending_resolutions(
    store: &DatabaseStore,
    limit: u32,
) -> Result<Vec<PendingResolution>, ConsignmentError> {
    let rows = sqlx::query(concat!(
        "SELECT consumption.id, consumption.intake_id ",
        pending_resolution_source!(),
        " ORDER BY consumption.consumed_at, consumption.id
         LIMIT $1",
    ))
    .bind(i64::from(limit))
    .fetch_all(store.pool())
    .await
    .map_err(|error| WriteError::from(error).into_application())?;
    rows.iter()
        .map(|row| {
            Ok(PendingResolution {
                consumption_id: row.try_get("id")?,
                intake_id: row.try_get("intake_id")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(|error| WriteError::from(error).into_application())
}
