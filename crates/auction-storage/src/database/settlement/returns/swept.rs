//! 结算退件 Take 结果未知（`take_unknown`）的机器可验证出口：邮箱巡检在原 Take 会话里观察到的
//! 精确事实，在巡检同一 Serializable 事务内推进案件。
//!
//! - 完成证明：integrations 在原 MID 详情页上核对了原发件人与主题、原退件正文前缀与逐条领取
//!   回执，且预期资产逐项一致。投递收为 succeeded 后经与正常收口同一个 [`consume_take_proof`]
//!   记账：领回事实与资金返还各一次，再按计划补寄或代管（不变量 1、3、8）。
//! - 仍在未领：原 MID 仍挂着完全相同的资产，说明 Take 没有生效；投递重新排队、动作回到 enqueued、
//!   案件回到 `take_pending`，由投递 Worker 在原租约与发送围栏协议下再执行一次。执行前的预检仍
//!   要求可领取且资产一致，已被领取的邮件不会被领取第二次；同邮件互斥在 world 锁下复核（不变量 4）。
//! - 原 Take 会话内的其他观察（邮件整体缺失、资产不全、回执不成立等）不改写投递、动作、资金或
//!   案件状态；邮箱维护另以 `take_observed` 事件只追加留证（`observed`），供详情与告警说明原因。
//!   换账号或换凭据的观察不是原会话，拒绝留证，也不改写任何事实。
//!
//! 锁序：调用方先锁观测 generation，本模块依次锁 world → 资金 scope/账户 → 恢复记录与义务；调用方
//! 锁凭据后再锁案件 → 动作，然后锁投递并改写投递，最后由本模块推进案件与履约（不变量 2）。

use auction_application::mail::MailDeliveryOrigin;
use auction_application::settlement::SettlementReturnProgress;
use auction_domain::{SettlementReturnCaseStatus, cash_return};
use auction_time::Timestamp;
use serde_json::json;
use sqlx::Postgres;
use uuid::Uuid;

use super::admission::obligation_kind;
use super::cases::{CaseEvent, CaseRow, CaseTransition, load_case, transition};
use super::errors::ReturnTransactionError;
use super::take::{
    ActionRow, TAKE_SWEEP_PROOF, TakeOutcome, load_action, observe_take, take_command_id,
    take_origin_id,
};
use super::work::{TakeProof, consume_take_proof};
use crate::database::TrackedTransaction as Transaction;
use crate::database::mail::delivery::disposition::settlement_return_take_conflicts;
use crate::database::settlement::fulfillment::{
    lock_settlement_delivery_eligibility, recovery_allows_execution_now,
};
use crate::database::settlement::funding::{LockedFundingAccount, lock_funded_delivery_account};

/// 巡检事务对一条 Take 投递的归属判定。
pub(crate) enum SweptTakeOwner {
    /// 不是结算退件 Take：按原入站恢复规则处理。
    Other,
    /// 是结算退件 Take，但来源、案件或绑定无法证明：本轮不改写任何事实。
    Unprovable,
    /// 已按锁序锁定 world、资金与恢复/义务前缀。
    Return(SweptTakePrefix),
}

/// 凭据锁之前已锁定的结算退件前缀。
pub(crate) struct SweptTakePrefix {
    return_mail_id: Uuid,
    take_delivery_id: Uuid,
    obligation_id: Uuid,
    original_delivery: Uuid,
    account: Option<LockedFundingAccount>,
    executable: bool,
}

/// 案件与动作也已锁定、确认停在结果未知的结算退件 Take。
pub(crate) struct SweptTake {
    prefix: SweptTakePrefix,
    case: CaseRow,
    action: ActionRow,
}

/// 判定 Take 投递是否属于结算退件，属于时锁定 world → 资金 → 恢复与义务前缀。来源字段不可变，
/// 无锁读取后再加锁不影响 Serializable 下的判定。
pub(crate) async fn swept_take_owner(
    tx: &mut Transaction<'_, Postgres>,
    take_delivery_id: Uuid,
) -> Result<SweptTakeOwner, ReturnTransactionError> {
    let origin: Option<(String, String)> =
        sqlx::query_as("SELECT origin_kind, origin_id FROM public.deliveries WHERE id = $1")
            .bind(take_delivery_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some((origin_kind, origin_id)) = origin else {
        return Ok(SweptTakeOwner::Other);
    };
    if origin_kind != MailDeliveryOrigin::SettlementReturnAction.as_str() {
        return Ok(SweptTakeOwner::Other);
    }
    // 来源只写规范案件 UUID；无法逐字还原即无法证明归属，失败关闭。
    let Some(return_mail_id) = origin_id
        .strip_prefix("settlement_return:")
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| take_origin_id(*value) == origin_id)
    else {
        return Ok(SweptTakeOwner::Unprovable);
    };
    let Some(snapshot) = load_case(tx, return_mail_id, false).await? else {
        return Ok(SweptTakeOwner::Unprovable);
    };
    let (Some(obligation_id), Some(original_delivery)) =
        (snapshot.obligation_id, snapshot.delivery_id)
    else {
        return Ok(SweptTakeOwner::Unprovable);
    };
    sqlx::query("SELECT id FROM public.worlds WHERE id = $1 FOR UPDATE")
        .bind(&snapshot.world_id)
        .execute(&mut **tx)
        .await?;
    let kind = obligation_kind(tx, Some(obligation_id)).await?;
    let account = if kind.is_some_and(cash_return) {
        lock_funded_delivery_account(tx, original_delivery).await?
    } else {
        None
    };
    lock_settlement_delivery_eligibility(tx, original_delivery).await?;
    let executable = recovery_allows_execution_now(tx, obligation_id).await?;
    Ok(SweptTakeOwner::Return(SweptTakePrefix {
        return_mail_id,
        take_delivery_id,
        obligation_id,
        original_delivery,
        account,
        executable,
    }))
}

/// 凭据锁之后、投递锁之前锁定案件与动作；只接受仍停在结果未知、动作在人工核对且正是这条 Take
/// 投递的案件。`take_pending` 的案件仍归正常收口（巡检不抢先改写），其余返回 `None`。
pub(crate) async fn lock_swept_take(
    tx: &mut Transaction<'_, Postgres>,
    prefix: SweptTakePrefix,
) -> Result<Option<SweptTake>, ReturnTransactionError> {
    let (Some(case), Some(action)) = (
        load_case(tx, prefix.return_mail_id, true).await?,
        load_action(tx, prefix.return_mail_id, true).await?,
    ) else {
        return Ok(None);
    };
    if case.status != SettlementReturnCaseStatus::TakeUnknown
        || case.plan.is_none()
        || case.obligation_id != Some(prefix.obligation_id)
        || case.delivery_id != Some(prefix.original_delivery)
        || action.execution_state != "manual_reconcile"
        || action.delivery_id != Some(prefix.take_delivery_id)
    {
        return Ok(None);
    }
    Ok(Some(SweptTake { prefix, case, action }))
}

/// 投递已按巡检完成证明收为 succeeded 之后：在锁内重读投递，只有证明形状正是巡检恢复且与动作
/// 冻结事实逐项一致时才按正常领回证明记账；否则返回 `None`，由调用方回滚整个事务。
pub(crate) async fn settle_swept_take(
    tx: &mut Transaction<'_, Postgres>,
    swept: &SweptTake,
) -> Result<Option<SettlementReturnProgress>, ReturnTransactionError> {
    let TakeOutcome::Proven { proof_digest, proof, .. } = observe_take(tx, &swept.action).await?
    else {
        return Ok(None);
    };
    if proof != TAKE_SWEEP_PROOF {
        return Ok(None);
    }
    let now = database_now(tx).await?;
    let proven = TakeProof {
        obligation_id: swept.prefix.obligation_id,
        original_delivery: swept.prefix.original_delivery,
        account: swept.prefix.account.as_ref(),
        executable: swept.prefix.executable,
        proof_digest: &proof_digest,
        proof,
    };
    consume_take_proof(tx, &swept.case, &swept.action, &proven, now).await.map(Some)
}

/// 投递已因“资产仍在、未被领取”重新排队之后：world 锁下复核同邮件互斥，再把动作收回 enqueued、
/// 案件收回 `take_pending`；有其他来源的处置意图时返回 `None`，由调用方回滚、保持结果未知。
pub(crate) async fn requeue_swept_take(
    tx: &mut Transaction<'_, Postgres>,
    swept: &SweptTake,
    sweep_retries: i64,
    reason: &str,
) -> Result<Option<SettlementReturnProgress>, ReturnTransactionError> {
    let action = &swept.action;
    if settlement_return_take_conflicts(
        tx,
        &action.world_id,
        &action.source_external_mail_id,
        take_command_id(action.return_mail_id).as_uuid(),
    )
    .await?
    {
        return Ok(None);
    }
    let now = database_now(tx).await?;
    let updated = sqlx::query(
        r#"UPDATE public.settlement_return_actions
           SET execution_state = 'enqueued',
               failure_code = NULL,
               completed_at = NULL,
               state_version = state_version + 1,
               updated_at = $4
           WHERE return_mail_id = $1 AND state_version = $2 AND delivery_id = $3
             AND execution_state = 'manual_reconcile'"#,
    )
    .bind(action.return_mail_id)
    .bind(action.state_version)
    .bind(swept.prefix.take_delivery_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(crate::database::repository_error("结算退件 Take 动作状态已变化").into());
    }
    let case: &CaseRow = &swept.case;
    let next = CaseTransition::from_case(case, SettlementReturnCaseStatus::TakePending);
    let version = transition(
        tx,
        case,
        &next,
        CaseEvent {
            kind: "take_activated",
            command: None,
            details: json!({
                "delivery_id": swept.prefix.take_delivery_id,
                "requeued": true,
                "reason": reason,
                "sweep_retries": sweep_retries,
            }),
        },
        now,
    )
    .await?;
    Ok(Some(SettlementReturnProgress {
        return_mail_id: case.return_mail_id,
        status: SettlementReturnCaseStatus::TakePending,
        case_version: version,
        event_kind: "take_activated",
    }))
}

/// 与投递改写同一时钟（`clock_timestamp()`），案件与事件时刻不早于投递终态。
async fn database_now(tx: &mut Transaction<'_, Postgres>) -> Result<Timestamp, sqlx::Error> {
    sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(&mut **tx).await
}
