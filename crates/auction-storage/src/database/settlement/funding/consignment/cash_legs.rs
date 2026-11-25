//! 寄售现金腿（载体本金 1、卖款 S、退款 P）的状态迁移：reserve / in_flight / unknown / spent / claimed / retained。
//!
//! 保护不变量 4、5、6：状态迁移只在同用途内移动已有金额、不增加资产；`spent` 只随已证外达，
//! 未知保留全部占用；外发结果未知不推导返还（返还只来自精确 Take 的回补，见 `returns.rs`）。
//! 每个迁移追加一条以原 delivery 为命令键的分录，返还时据此定位原支出。

use auction_application::settlement::{
    CashLeg, CashPhase, CashReceipt, FundingError, FundingMutation, SettleCash,
    SettlementInternalEvidenceSnapshot,
};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::commands::{record_command, replay_command};
use super::contract_allocations::{
    AllocationRow, contract_allocation, locked_allocation, sale_facts,
};
use super::order_stage::record_stage_change;
use super::record_only::check_mode;
use super::transaction::run_serializable;
use super::world_leg::checked_payment_leg;
use crate::DatabaseStore;
use crate::database::TrackedTransaction as Transaction;
use crate::database::consignment::dispatch::claim_proof::cash_claim_signed;
use crate::database::settlement::funding::account::{
    LockedFundingAccount, check_funding_evidence, lock_funding_account, verify_locked,
};
use crate::database::settlement::funding::ledger::{
    FundingEntry, allocation, complete, entry, issue, source, transfer,
};
use crate::database::settlement::funding::returns::record_delivery_binding;
use crate::database::settlement::funding::{lock_funding_worlds, storage};

/// 载体本金固定 1 Credit。
const CARRIER_PRINCIPAL: i64 = 1;

const fn leg_purpose(leg: CashLeg) -> &'static str {
    match leg {
        CashLeg::CarrierPrincipal => "carrier",
        CashLeg::Seller => "seller",
        CashLeg::Refund => "refund",
    }
}

/// 该腿对应的寄售义务种类。
const fn leg_obligation_kind(leg: CashLeg) -> &'static str {
    match leg {
        CashLeg::CarrierPrincipal => "winner_payment_carrier_cod",
        CashLeg::Seller => "seller_payout",
        CashLeg::Refund => "buyer_refund",
    }
}

const fn phase_action(phase: CashPhase) -> &'static str {
    match phase {
        CashPhase::Reserve => "consignment_cash_reserve",
        CashPhase::InFlight => "consignment_cash_in_flight",
        CashPhase::Unknown => "consignment_cash_unknown",
        CashPhase::Spent => "consignment_cash_spent",
        CashPhase::Claimed => "consignment_cash_claimed",
        CashPhase::Retained => "consignment_cash_retained",
    }
}

/// 现金腿状态迁移的事务入口：同命令 ID 同请求返回原结果，同 ID 异请求冲突。
pub(crate) async fn settle_cash(
    store: &DatabaseStore,
    command: SettleCash,
) -> Result<CashReceipt, FundingError> {
    if command.contract_id.is_nil()
        || command.obligation_id.is_nil()
        || command.delivery_id.is_nil()
        || command.amount <= 0
    {
        return Err(FundingError::Invalid);
    }
    run_serializable(store, &command, |tx, command| Box::pin(settle_cash_in_tx(tx, command))).await
}

/// 事务体：锁序 funding worlds → UID 账户 → 义务行 → 分配 → 投递绑定 → 权利锁（阶段变更入口）。
async fn settle_cash_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    command: &SettleCash,
) -> Result<CashReceipt, FundingError> {
    let sale = sale_facts(tx, command.contract_id).await?;
    lock_funding_worlds(tx).await?;
    let account =
        lock_funding_account(tx, &sale.funding_uid).await?.ok_or(FundingError::Unavailable)?;
    let action = phase_action(command.phase);
    if replay_command(tx, account.uid(), &command.mutation, action, command.delivery_id).await? {
        return Ok(CashReceipt { replayed: true });
    }
    bind_round(tx, command).await?;
    let uid = account.uid().to_owned();
    // 状态迁移的分录以原 delivery 为命令键（与拍卖一致），返还据此定位原支出。
    let moved = FundingMutation {
        command_id: command.delivery_id,
        request_digest: command.mutation.request_digest,
    };
    let changed = match command.phase {
        CashPhase::Reserve => reserve(tx, &account, command, &moved).await?,
        CashPhase::InFlight => in_flight(tx, &account, command, &moved).await?,
        CashPhase::Unknown => finish(tx, &uid, command, &moved, "unknown").await?,
        CashPhase::Spent => finish(tx, &uid, command, &moved, "spent").await?,
        CashPhase::Claimed => {
            require_cash_claim_proof(tx, command.delivery_id).await?;
            claimed(tx, &uid, command, &moved).await?
        }
        CashPhase::Retained => retained(tx, &uid, command, &moved).await?,
    };
    if !changed {
        // 不变量 1：没有状态变化的命令同样登记账本，之后同 ID 异请求才会冲突；
        // 不推进账户版本（没有资金事实变化，不能作废别人持有的预览）。
        record_command(tx, account.uid(), &command.mutation, action, command.delivery_id).await?;
        return Ok(CashReceipt { replayed: true });
    }
    complete(tx, &account, &command.mutation, action, command.delivery_id).await?;
    // 不变量 3：只有确有资金事实变化的迁移追加订单阶段事件并刷新权利，与迁移同事务；
    // 重放与无变化命令在上面已返回，不重复追加。
    record_stage_change(tx, command.contract_id).await?;
    Ok(CashReceipt { replayed: false })
}

/// 寄售派发轮现金腿（载体本金 1、卖款 S、退款 P）在派发事务内的资金身份：合同、现金义务、轮投递、
/// 该腿、冻结金额与派发命令的账本身份。
#[derive(Debug, Clone)]
pub(crate) struct DispatchCash {
    /// 派发命令（起写或 Sent 证据）的命令 ID 与摘要，由应用层派生；资金命令账本沿用它。
    pub(crate) mutation: FundingMutation,
    pub(crate) contract_id: Uuid,
    pub(crate) obligation_id: Uuid,
    pub(crate) delivery_id: Uuid,
    pub(crate) leg: CashLeg,
    /// 轮上冻结的 Credits；`bind_round` 在义务锁下复核它等于轮冻结值、义务种类与腿一致。
    pub(crate) amount: i64,
}

/// 起写同事务的资金命令动作：reserve 与 in_flight 作为一次命令提交。
const DISPATCH_START_ACTION: &str = "consignment_dispatch_cash_write_start";

impl DispatchCash {
    fn cash(&self, phase: CashPhase) -> SettleCash {
        SettleCash {
            mutation: self.mutation.clone(),
            contract_id: self.contract_id,
            obligation_id: self.obligation_id,
            delivery_id: self.delivery_id,
            leg: self.leg,
            phase,
            amount: self.amount,
        }
    }
}

/// 派发现金腿的资金锁前缀：funding worlds → 成交冻结的 UID 账户。
///
/// 派发事务在取任何业务行锁之前调用（D9 §1.3：`runtime generation → funding worlds → UID 账户 →
/// 来源业务行 → …`），与通用现金起写的账户锁位置一致。账户不存在失败关闭（不变量 6）。
pub(crate) async fn lock_dispatch_funding_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    contract_id: Uuid,
) -> Result<(), FundingError> {
    let sale = sale_facts(tx, contract_id).await?;
    lock_funding_worlds(tx).await?;
    lock_funding_account(tx, &sale.funding_uid).await?.ok_or(FundingError::Unavailable)?;
    Ok(())
}

/// 现金腿首次起写同事务的资金占用：reserve → in_flight（D9 §4.3–4.4）。
///
/// 保护不变量 2、4、6：调用方已按 [`lock_dispatch_funding_in_tx`] 取得资金锁前缀，并在受益用户之后、投递之前
/// 锁住该现金义务行；本函数复用 `settle_cash` 的同一组迁移（载体本金在严格现金下由原 UID 自有借出 1、
/// record_only 由 `record_carrier` 发行 1；卖款与退款占用已在用途里的 S / P），账户模式必须等于成交冻结的
/// 模式。余额不足、账户未就绪或占用状态不符时返回错误，调用方整笔回滚、不发起写许可。订单阶段事件由起写
/// 事务自己追加，这里不重复。
///
/// 现金外发前另按拍卖通用现金门（[`check_funding_evidence`]，同一实现）核对发信会话里取得的新鲜证据：认证
/// UID 必须是成交冻结的资金 UID，证据须在数据库时钟下仍有效；严格现金再要求观察余额覆盖该 UID 全部未支出
/// 占用、且没有未收口的外发，记账模式不读余额。证据本身不成立（UID 不符、过期等）返回错误，整笔回滚。
///
/// 返回 `Ok(false)` 表示现金门不放行（账实缺口或该 UID 有未收口外发）：通用现金门已在本事务写入缺口时的
/// 账户冻结，调用方必须提交这份冻结、不写任何起写事实与账本回执、不发许可（与拍卖同口径，裁定
/// 2026-11-25；不变量 4、6）。`Ok(true)` 表示已占用（或重放）。
pub(crate) async fn start_dispatch_cash_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    dispatch: &DispatchCash,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<bool, FundingError> {
    let command = dispatch.cash(CashPhase::Reserve);
    let (account, moved) = dispatch_account(tx, &command).await?;
    if replay_command(
        tx,
        account.uid(),
        &command.mutation,
        DISPATCH_START_ACTION,
        dispatch.delivery_id,
    )
    .await?
    {
        return Ok(true);
    }
    // 现金门在冻结金额绑定之前、任何占用之前：余额只与该 UID 已有的未支出占用比较（与拍卖同口径）。
    if !check_funding_evidence(tx, &account, evidence).await? {
        return Ok(false);
    }
    bind_round(tx, &command).await?;
    reserve(tx, &account, &command, &moved).await?;
    if !in_flight(tx, &account, &command, &moved).await? {
        // 刚预留的占用不可能已在途：出现即状态损坏，失败关闭。
        return Err(FundingError::Conflict);
    }
    complete(tx, &account, &command.mutation, DISPATCH_START_ACTION, dispatch.delivery_id).await?;
    Ok(true)
}

/// 现金腿的 Sent 绑定同事务：占用 in_flight / unknown → spent（已证外达，D9 §4.3）。
///
/// 保护不变量 3、4、6：只随本事务新写入的 Sent 绑定证据调用；占用必须绑定该投递，缺占用或状态不符失败关闭，
/// 已是 spent 只登记命令。订单阶段事件由证据事务自己追加，这里不重复。
pub(crate) async fn settle_dispatch_cash_sent_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    dispatch: &DispatchCash,
) -> Result<(), FundingError> {
    let command = dispatch.cash(CashPhase::Spent);
    let (account, moved) = dispatch_account(tx, &command).await?;
    let action = phase_action(CashPhase::Spent);
    if replay_command(tx, account.uid(), &command.mutation, action, dispatch.delivery_id).await? {
        return Ok(());
    }
    bind_round(tx, &command).await?;
    if finish(tx, account.uid(), &command, &moved, "spent").await? {
        complete(tx, &account, &command.mutation, action, dispatch.delivery_id).await
    } else {
        record_command(tx, account.uid(), &command.mutation, action, dispatch.delivery_id).await
    }
}

/// 现金确领证据同事务：已支出的卖款 S / 退款 P 追加一条 claimed（D9 §2.3 `remote_cash_claimed`、§4.3）。
///
/// 保护不变量 3、4、6：只随本事务新写入、且出自已签收验证器的现金确领证据调用；占用必须仍是绑定该投递的
/// `spent`（已证外达），否则失败关闭、整笔回滚（证据不落）。已记 claimed 只登记命令。订单阶段事件由证据事务
/// 自己追加，这里不重复。
pub(crate) async fn settle_dispatch_cash_claimed_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    dispatch: &DispatchCash,
) -> Result<(), FundingError> {
    let command = dispatch.cash(CashPhase::Claimed);
    let (account, moved) = dispatch_account(tx, &command).await?;
    let action = phase_action(CashPhase::Claimed);
    if replay_command(tx, account.uid(), &command.mutation, action, dispatch.delivery_id).await? {
        return Ok(());
    }
    bind_round(tx, &command).await?;
    if claimed(tx, account.uid(), &command, &moved).await? {
        complete(tx, &account, &command.mutation, action, dispatch.delivery_id).await
    } else {
        record_command(tx, account.uid(), &command.mutation, action, dispatch.delivery_id).await
    }
}

/// 无 COD 现金 `claimed` 必引远端确领（D9 §6.2）：该投递已绑定 Sent 的尝试上须有出自已签收验证器的现金确领
/// 证据；没有即未证明，失败关闭（不变量 6）。
async fn require_cash_claim_proof(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<(), FundingError> {
    let versions: Vec<String> = sqlx::query_scalar(
        r#"SELECT evidence.validator_version
           FROM public.consignment_dispatch_evidence AS evidence
           JOIN public.consignment_dispatch_attempts AS attempt
             ON attempt.attempt_id = evidence.attempt_id
           JOIN public.delivery_sent_bindings AS binding
             ON binding.delivery_id = attempt.delivery_id
            AND binding.fencing_version = attempt.fence
           WHERE attempt.delivery_id = $1 AND evidence.category = 'cash_claim'"#,
    )
    .bind(delivery_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    if versions.iter().any(|version| cash_claim_signed(version)) {
        Ok(())
    } else {
        Err(FundingError::Unproven)
    }
}

/// 重取（同事务内已持有）成交 UID 的账户锁，核对账户模式等于成交冻结的模式，并给出以原 delivery 为命令键的分录身份。
async fn dispatch_account(
    tx: &mut Transaction<'_, Postgres>,
    command: &SettleCash,
) -> Result<(LockedFundingAccount, FundingMutation), FundingError> {
    let sale = sale_facts(tx, command.contract_id).await?;
    let account =
        lock_funding_account(tx, &sale.funding_uid).await?.ok_or(FundingError::Unavailable)?;
    check_mode(&account, sale.funding_mode)?;
    let moved = FundingMutation {
        command_id: command.delivery_id,
        request_digest: command.mutation.request_digest,
    };
    Ok((account, moved))
}

/// 命令引用的 delivery 必须是该合同、该义务的冻结派发轮，且义务种类与腿、冻结金额与命令一致。
async fn bind_round(
    tx: &mut Transaction<'_, Postgres>,
    command: &SettleCash,
) -> Result<(), FundingError> {
    let row = sqlx::query(
        r#"SELECT round.contract_id, round.obligation_id, round.credits, obligation.obligation_kind
           FROM public.consignment_dispatch_rounds AS round
           JOIN public.settlement_obligations AS obligation
             ON obligation.id = round.obligation_id AND obligation.contract_id = round.contract_id
           WHERE round.delivery_id=$1
           FOR UPDATE OF obligation"#,
    )
    .bind(command.delivery_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    if row.try_get::<Uuid, _>("contract_id").map_err(storage)? != command.contract_id
        || row.try_get::<Uuid, _>("obligation_id").map_err(storage)? != command.obligation_id
        || row.try_get::<String, _>("obligation_kind").map_err(storage)?
            != leg_obligation_kind(command.leg)
        || row.try_get::<i64, _>("credits").map_err(storage)? != command.amount
    {
        return Err(FundingError::Conflict);
    }
    // 世界与收款腿互校：载体本金腿的义务必须是其世界对应的载体付款腿（与 `CertifyCod::check_leg` 同源）。
    if command.leg == CashLeg::CarrierPrincipal {
        checked_payment_leg(tx, command.obligation_id, command.contract_id).await?;
    }
    Ok(())
}

async fn leg_allocation(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    command: &SettleCash,
) -> Result<AllocationRow, FundingError> {
    locked_allocation(tx, uid, leg_purpose(command.leg), command.contract_id)
        .await?
        .ok_or(FundingError::Unavailable)
}

/// 同用途内 available → reserved 并登记 delivery 绑定；载体本金在严格现金下先由自有借出 1。
async fn reserve(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &SettleCash,
    moved: &FundingMutation,
) -> Result<bool, FundingError> {
    if !account.ready() {
        return Err(FundingError::Unavailable);
    }
    let uid = account.uid();
    if command.leg == CashLeg::CarrierPrincipal {
        let funded = locked_allocation(tx, uid, "carrier", command.contract_id)
            .await?
            .is_some_and(|row| row.amount > 0);
        if !funded {
            let carrier = contract_allocation(
                tx,
                uid,
                "carrier",
                command.contract_id,
                Some(command.obligation_id),
            )
            .await?;
            if account.records_only() {
                // record_only 无个人余额可借：首封由专用 record_carrier 来源向 carrier 发行 1（D9 §4.4）。
                // 来源形状（寄售 isekai 载体义务、main 的冻结派发轮、同合同同义务的 carrier 分配）
                // 由库内 enforce_settlement_funding_issue_limit_000120 的寄售分支校验。
                let source = source(
                    tx,
                    uid,
                    "record_carrier",
                    command.delivery_id,
                    CARRIER_PRINCIPAL,
                    &moved.request_digest,
                )
                .await?;
                issue(tx, uid, moved, source, carrier, CARRIER_PRINCIPAL).await?;
            } else {
                let proprietary = allocation(tx, uid, "proprietary", None, None).await?;
                transfer(tx, uid, moved, proprietary, carrier, command.amount).await?;
            }
        }
    }
    let row = leg_allocation(tx, uid, command).await?;
    if row.amount != command.amount {
        return Err(FundingError::Conflict);
    }
    let reserved = sqlx::query(
        r#"UPDATE public.settlement_funding_allocations
           SET state='reserved', delivery_id=$1, state_version=state_version+1
           WHERE id=$2 AND uid=$3 AND state='available' AND amount=$4
             AND delivery_id IS NULL"#,
    )
    .bind(command.delivery_id)
    .bind(row.id)
    .bind(uid)
    .bind(command.amount)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    if reserved.rows_affected() != 1 {
        return Err(FundingError::Unavailable);
    }
    record_delivery_binding(tx, account, command.delivery_id, row.id).await?;
    state_entry(tx, uid, moved, row.id, command.amount, "reserve").await?;
    Ok(true)
}

/// reserved → in_flight；必须在账户 ready 时、与发送 fence 同事务由调用方推进。
async fn in_flight(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &SettleCash,
    moved: &FundingMutation,
) -> Result<bool, FundingError> {
    verify_locked(tx, account, true).await?;
    let uid = account.uid();
    let row = leg_allocation(tx, uid, command).await?;
    if row.delivery_id != Some(command.delivery_id) || row.amount != command.amount {
        return Err(FundingError::Conflict);
    }
    match row.state.as_str() {
        "in_flight" => return Ok(false),
        "reserved" => {}
        _ => return Err(FundingError::Conflict),
    }
    set_state(tx, uid, row.id, "in_flight").await?;
    state_entry(tx, uid, moved, row.id, command.amount, "in_flight").await?;
    Ok(true)
}

/// unknown / spent：已外发的结果只能前进；`spent` 不得从 reserved 直接到达（未起写不能算外达）。
async fn finish(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    command: &SettleCash,
    moved: &FundingMutation,
    state: &str,
) -> Result<bool, FundingError> {
    let row = leg_allocation(tx, uid, command).await?;
    if row.delivery_id != Some(command.delivery_id) || row.amount != command.amount {
        return Err(FundingError::Conflict);
    }
    if row.state == state {
        return Ok(false);
    }
    let allowed = match state {
        "spent" => matches!(row.state.as_str(), "in_flight" | "unknown"),
        _ => matches!(row.state.as_str(), "reserved" | "in_flight"),
    };
    if !allowed {
        return Err(FundingError::Conflict);
    }
    set_state(tx, uid, row.id, state).await?;
    state_entry(tx, uid, moved, row.id, command.amount, state).await?;
    Ok(true)
}

/// 已证现金确领：只追加一条 claimed 分录，分配仍是 spent，金额、受益人与派发逐项相等。
async fn claimed(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    command: &SettleCash,
    moved: &FundingMutation,
) -> Result<bool, FundingError> {
    let row = leg_allocation(tx, uid, command).await?;
    if row.state != "spent"
        || row.delivery_id != Some(command.delivery_id)
        || row.amount != command.amount
    {
        return Err(FundingError::Conflict);
    }
    let recorded: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
               SELECT 1 FROM public.settlement_funding_entries
               WHERE uid=$1 AND kind='claimed' AND debit_id=$2 AND command_id=$3
           )"#,
    )
    .bind(uid)
    .bind(row.id)
    .bind(command.delivery_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if recorded {
        return Ok(false);
    }
    state_entry(tx, uid, moved, row.id, command.amount, "claimed").await?;
    Ok(true)
}

/// 原受益用途 → 同权利 retained：只转移仍 available 的金额，已派发或在途的钱不能留存。
async fn retained(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    command: &SettleCash,
    moved: &FundingMutation,
) -> Result<bool, FundingError> {
    let row = leg_allocation(tx, uid, command).await?;
    if row.state != "available" || row.delivery_id.is_some() || row.amount < command.amount {
        return Err(FundingError::Conflict);
    }
    let retained = contract_allocation(tx, uid, "retained", command.contract_id, None).await?;
    transfer(tx, uid, moved, row.id, retained, command.amount).await?;
    Ok(true)
}

async fn set_state(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    allocation_id: Uuid,
    state: &str,
) -> Result<(), FundingError> {
    let changed = sqlx::query(
        r#"UPDATE public.settlement_funding_allocations
           SET state=$1, state_version=state_version+1
           WHERE id=$2 AND uid=$3"#,
    )
    .bind(state)
    .bind(allocation_id)
    .bind(uid)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    if changed.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    Ok(())
}

async fn state_entry(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    moved: &FundingMutation,
    allocation_id: Uuid,
    amount: i64,
    kind: &str,
) -> Result<(), FundingError> {
    entry(
        tx,
        uid,
        moved,
        FundingEntry {
            debit: Some(allocation_id),
            credit: Some(allocation_id),
            source: None,
            amount,
            kind,
        },
    )
    .await
}
