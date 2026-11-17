//! Worker 推进持久 Take 的两个本地事务：激活到期 Take、收口已到终态的 Take。
//!
//! 锁序沿用既有业务锁的完整前缀：激活为 world → 案件 → 动作 → 投递；收口为资金 scope/账户 →
//! 恢复记录与义务 → 案件 → 动作 → 投递 → 履约，各集合只取单行，不反向取锁（不变量 2）。
//! 领取证明成立后的记账（领回事实、资金返还、补寄或代管）只有 [`consume_take_proof`] 一处实现，
//! 正常收口与邮箱巡检恢复共用。确定未外写的 Take 重启与激活前冲突的交接见 `restart`。

use auction_application::core::ApplicationError;
use auction_application::mail::MailDeliveryOrigin;
use auction_application::settlement::{FundingMutation, SettlementReturnProgress};
use auction_domain::{
    SettlementReturnBlocker, SettlementReturnCaseStatus, SettlementReturnPlan, cash_return,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::Postgres;
use uuid::Uuid;

use super::admission::{HELD_FAILURE_CODE, admission_facts, obligation_kind};
use super::cases::{CaseEvent, CaseRow, CaseTransition, load_case, transition};
use super::errors::ReturnTransactionError;
use super::resend::{authorize_resend, close_fulfillment};
use super::restart::{hand_over_conflicted_take, restart_unstarted_take};
use super::take::{
    ActionRow, TakeOutcome, load_action, observe_take, take_activation, take_command,
    take_funding_command_id,
};
use crate::database::TrackedTransaction as Transaction;
use crate::database::mail::delivery::{
    activate_in_transaction, enqueue_in_transaction, return_take_blocked_sql,
    settlement_return_take_conflicts,
};
use crate::database::settlement::fulfillment::{
    lock_settlement_delivery_eligibility, recovery_allows_execution_now,
};
use crate::database::settlement::funding::{
    FundingReturnBinding, LockedFundingAccount, funded_delivery_spent,
    lock_funded_delivery_account, return_funded_delivery,
};

/// 激活一项到期的持久 Take，或把已没有阻断意图的激活前冲突案件交接回 Take 待执行；没有时返回
/// `None`。仍被阻断的冲突案件不进入候选，不会挡住其后的激活。只考虑 `take_worlds`（本进程能以
/// 持久凭据身份执行 Take 的世界）里的案件：其他世界的 Take 不激活，案件停在待领回，详情显示
/// “领回能力不可用”。
pub(super) async fn activate_due_take_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
    take_worlds: &[String],
) -> Result<Option<SettlementReturnProgress>, ApplicationError> {
    if take_worlds.iter().any(|world| !matches!(world.as_str(), "main" | "isekai")) {
        return Err(crate::database::repository_error("结算退件 Take 世界参数无效").into());
    }
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Fulfillment,
        crate::database::SERIALIZABLE_ATTEMPTS,
        ReturnTransactionError::retryable,
        || activate_once(pool, now, take_worlds),
    )
    .await
    .map_err(ReturnTransactionError::into_application)
}

async fn activate_once(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
    take_worlds: &[String],
) -> Result<Option<SettlementReturnProgress>, ReturnTransactionError> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    let candidate: Option<(Uuid, String)> = sqlx::query_as(concat!(
        r#"SELECT action.return_mail_id, action.world_id
           FROM public.settlement_return_actions AS action
           JOIN public.settlement_return_cases AS c ON c.return_mail_id = action.return_mail_id
           WHERE action.execution_state = 'pending'
             AND action.world_id = ANY($1::text[])
             AND (c.status = 'take_pending'
                  OR (c.status = 'take_unknown' AND NOT "#,
        return_take_blocked_sql!("action.world_id", "action.source_external_mail_id"),
        r#"))
           ORDER BY action.created_at, action.return_mail_id
           LIMIT 1"#
    ))
    .bind(take_worlds)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((return_mail_id, world_id)) = candidate else {
        tx.commit().await?;
        return Ok(None);
    };
    sqlx::query("SELECT id FROM public.worlds WHERE id = $1 FOR UPDATE")
        .bind(&world_id)
        .execute(&mut *tx)
        .await?;
    let (Some(case), Some(action)) = (
        load_case(&mut tx, return_mail_id, true).await?,
        load_action(&mut tx, return_mail_id, true).await?,
    ) else {
        tx.commit().await?;
        return Ok(None);
    };
    if case.status == SettlementReturnCaseStatus::TakeUnknown {
        let progress = hand_over_conflicted_take(&mut tx, &case, &action, now).await?;
        tx.commit().await?;
        return Ok(progress);
    }
    if case.status != SettlementReturnCaseStatus::TakePending || action.execution_state != "pending"
    {
        tx.commit().await?;
        return Ok(None);
    }
    let command = take_command(&action)?;
    // 同一邮件已有其他来源的领取或退回意图：不写入投递，转人工核对（失败、未知都不解除互斥）。
    if settlement_return_take_conflicts(
        &mut tx,
        &action.world_id,
        &action.source_external_mail_id,
        command.command_id.as_uuid(),
    )
    .await?
    {
        let progress =
            mark_take_unknown(&mut tx, &case, &action, "mail_disposition_conflict", now).await?;
        tx.commit().await?;
        return Ok(Some(progress));
    }
    let prepared = enqueue_in_transaction(&mut tx, &command, Some("prepared"), None).await?;
    // delivery owner 以自身时钟写入创建时刻；激活时刻不得早于它（updated_at >= created_at）。
    let now = now.max(Utc::now());
    let activation = take_activation(&action, prepared.delivery_id, command.request_hash, now)?;
    activate_in_transaction(&mut tx, &activation, MailDeliveryOrigin::SettlementReturnAction)
        .await?;
    let updated = sqlx::query(
        r#"UPDATE public.settlement_return_actions
           SET execution_state = 'enqueued',
               delivery_id = $3,
               prepare_command_id = $4,
               prepare_request_digest = $5,
               activate_command_id = $6,
               activate_request_digest = $7,
               state_version = state_version + 1,
               updated_at = $8
           WHERE return_mail_id = $1 AND state_version = $2 AND execution_state = 'pending'"#,
    )
    .bind(return_mail_id)
    .bind(action.state_version)
    .bind(prepared.delivery_id)
    .bind(activation.prepare_command_id.as_uuid())
    .bind(activation.prepare_request_hash.as_bytes().as_slice())
    .bind(activation.activate_command_id.as_uuid())
    .bind(activation.activate_request_hash.as_bytes().as_slice())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(crate::database::repository_error("结算退件 Take 动作状态已变化").into());
    }
    let next = CaseTransition::from_case(&case, SettlementReturnCaseStatus::TakePending);
    let version = transition(
        &mut tx,
        &case,
        &next,
        CaseEvent {
            kind: "take_activated",
            command: None,
            details: json!({"delivery_id": prepared.delivery_id}),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(Some(SettlementReturnProgress {
        return_mail_id,
        status: SettlementReturnCaseStatus::TakePending,
        case_version: version,
        event_kind: "take_activated",
    }))
}

/// Take 失败或结果未知（含确定未外写但重启次数已用满）：动作转人工核对、案件停在
/// `take_unknown`；不自动重试，不释放互斥。
/// 激活前即发现同邮件冲突时 Take 从未写入投递：动作保持没有投递身份的 pending（动作约束要求
/// 人工核对必有投递），原因只记在案件事件里；案件离开 `take_pending` 后不会再被激活。
async fn mark_take_unknown(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    action: &ActionRow,
    failure_code: &'static str,
    now: DateTime<Utc>,
) -> Result<SettlementReturnProgress, ReturnTransactionError> {
    if action.delivery_id.is_some() {
        let updated = sqlx::query(
            r#"UPDATE public.settlement_return_actions
               SET execution_state = 'manual_reconcile',
                   failure_code = $3,
                   completed_at = $4,
                   state_version = state_version + 1,
                   updated_at = $4
               WHERE return_mail_id = $1 AND state_version = $2
                 AND execution_state = 'enqueued'"#,
        )
        .bind(action.return_mail_id)
        .bind(action.state_version)
        .bind(failure_code)
        .bind(now)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(crate::database::repository_error("结算退件 Take 动作状态已变化").into());
        }
    } else if action.execution_state != "pending" {
        return Err(crate::database::repository_error("结算退件 Take 动作状态已变化").into());
    }
    let next = CaseTransition::from_case(case, SettlementReturnCaseStatus::TakeUnknown);
    let version = transition(
        tx,
        case,
        &next,
        CaseEvent {
            kind: "take_unknown",
            command: None,
            details: json!({"failure_code": failure_code, "delivery_id": action.delivery_id}),
        },
        now,
    )
    .await?;
    Ok(SettlementReturnProgress {
        return_mail_id: case.return_mail_id,
        status: SettlementReturnCaseStatus::TakeUnknown,
        case_version: version,
        event_kind: "take_unknown",
    })
}

/// 收口一项 Take 投递已到终态的动作；没有时返回 `None`。
pub(super) async fn settle_due_take_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<SettlementReturnProgress>, ApplicationError> {
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Fulfillment,
        crate::database::SERIALIZABLE_ATTEMPTS,
        ReturnTransactionError::retryable,
        || settle_once(pool, now),
    )
    .await
    .map_err(ReturnTransactionError::into_application)
}

async fn settle_once(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<SettlementReturnProgress>, ReturnTransactionError> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    let candidate: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT action.return_mail_id
           FROM public.settlement_return_actions AS action
           JOIN public.settlement_return_cases AS c ON c.return_mail_id = action.return_mail_id
           JOIN public.deliveries AS delivery ON delivery.id = action.delivery_id
           WHERE action.execution_state = 'enqueued'
             AND c.status = 'take_pending'
             AND delivery.status IN ('succeeded', 'failed', 'manual_reconcile')
           ORDER BY action.updated_at, action.return_mail_id
           LIMIT 1"#,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(return_mail_id) = candidate else {
        tx.commit().await?;
        return Ok(None);
    };
    let Some(snapshot) = load_case(&mut tx, return_mail_id, false).await? else {
        tx.commit().await?;
        return Ok(None);
    };
    let (Some(obligation_id), Some(original_delivery)) =
        (snapshot.obligation_id, snapshot.delivery_id)
    else {
        return Err(crate::database::repository_error("结算退件 Take 案件缺少绑定").into());
    };
    let kind = obligation_kind(&mut tx, snapshot.obligation_id).await?;
    // 锁序前缀：资金 scope/账户 → 恢复记录与义务 → 案件 → 动作 → 投递 → 履约。
    let account = if kind.is_some_and(cash_return) {
        lock_funded_delivery_account(&mut tx, original_delivery).await?
    } else {
        None
    };
    lock_settlement_delivery_eligibility(&mut tx, original_delivery).await?;
    let executable = recovery_allows_execution_now(&mut tx, obligation_id).await?;
    let (Some(case), Some(action)) = (
        load_case(&mut tx, return_mail_id, true).await?,
        load_action(&mut tx, return_mail_id, true).await?,
    ) else {
        tx.commit().await?;
        return Ok(None);
    };
    if case.status != SettlementReturnCaseStatus::TakePending
        || action.execution_state != "enqueued"
    {
        tx.commit().await?;
        return Ok(None);
    }
    let progress = match observe_take(&mut tx, &action).await? {
        TakeOutcome::Pending => {
            tx.commit().await?;
            return Ok(None);
        }
        TakeOutcome::NotStarted { failure_code } => {
            match restart_unstarted_take(&mut tx, &case, &action, now).await? {
                Some(progress) => progress,
                None => mark_take_unknown(&mut tx, &case, &action, failure_code, now).await?,
            }
        }
        TakeOutcome::Failed { failure_code } | TakeOutcome::Unknown { failure_code } => {
            mark_take_unknown(&mut tx, &case, &action, failure_code, now).await?
        }
        TakeOutcome::Proven { proof_digest, proof, .. } => {
            let proven = TakeProof {
                obligation_id,
                original_delivery,
                account: account.as_ref(),
                executable,
                proof_digest: &proof_digest,
                proof,
            };
            consume_take_proof(&mut tx, &case, &action, &proven, now).await?
        }
    };
    tx.commit().await?;
    Ok(Some(progress))
}

/// 已在锁内判定成立的一份领取证明及其记账范围。
pub(super) struct TakeProof<'a> {
    pub(super) obligation_id: Uuid,
    /// 被退回的原派发投递。
    pub(super) original_delivery: Uuid,
    /// 现金退件时按锁序已锁定的原资金账户；装备退件或历史无资金分配时为空。
    pub(super) account: Option<&'a LockedFundingAccount>,
    /// 恢复谓词：原义务此刻仍可执行。
    pub(super) executable: bool,
    pub(super) proof_digest: &'a [u8; 32],
    /// 证明形状（成功页回执或巡检恢复），写进 `take_proven` 事件。
    pub(super) proof: &'static str,
}

/// 领取证明成立后的唯一记账路径：只追加领回事实（同案件、Take 投递、原派发与邮件各一次），现金
/// 退件同事务把原分配 spent → available 返还一次（金额仍为原额，不是 2M），再按计划补寄或代管
/// （不变量 1、3、8）。调用方已持有资金 → 恢复与义务 → 案件 → 动作 → 投递的锁。
pub(super) async fn consume_take_proof(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    action: &ActionRow,
    proven: &TakeProof<'_>,
    now: DateTime<Utc>,
) -> Result<SettlementReturnProgress, ReturnTransactionError> {
    record_take(tx, case, action, proven, now).await?;
    if let Some(account) = proven.account
        && funded_delivery_spent(tx, account, proven.original_delivery).await?
    {
        return_funded_delivery(
            tx,
            account,
            &FundingMutation {
                command_id: take_funding_command_id(case.return_mail_id),
                request_digest: *proven.proof_digest,
            },
            proven.original_delivery,
            FundingReturnBinding {
                return_mail_id: case.return_mail_id,
                take_delivery_id: action
                    .delivery_id
                    .ok_or_else(|| crate::database::repository_error("Take 投递缺失"))?,
                credits: action.expected_credits,
                proof_digest: proven.proof_digest,
            },
        )
        .await?;
    }
    dispose_taken(tx, case, Some(proven.executable), proven.proof, now).await
}

/// 只追加领回事实并把动作收为 satisfied；同一案件、Take 投递、原派发与邮件各只能领回一次。
/// 动作可来自 enqueued（正常收口）或 manual_reconcile（巡检恢复），两者都以版本比较交换推进。
async fn record_take(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    action: &ActionRow,
    proven: &TakeProof<'_>,
    now: DateTime<Utc>,
) -> Result<(), ReturnTransactionError> {
    let take_delivery =
        action.delivery_id.ok_or_else(|| crate::database::repository_error("Take 投递缺失"))?;
    sqlx::query(
        r#"INSERT INTO public.settlement_return_takes
               (return_mail_id, take_delivery_id, original_delivery_id, obligation_id, world_id,
                external_mail_id, credits, cod, attachment_ids, proof_digest, proven_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
    )
    .bind(case.return_mail_id)
    .bind(take_delivery)
    .bind(proven.original_delivery)
    .bind(proven.obligation_id)
    .bind(&action.world_id)
    .bind(&action.source_external_mail_id)
    .bind(action.expected_credits)
    .bind(action.expected_cod)
    .bind(&action.expected_attachment_ids)
    .bind(proven.proof_digest.as_slice())
    .bind(now)
    .execute(&mut **tx)
    .await?;
    let updated = sqlx::query(
        r#"UPDATE public.settlement_return_actions
           SET execution_state = 'satisfied',
               failure_code = NULL,
               completed_at = $3,
               state_version = state_version + 1,
               updated_at = $3
           WHERE return_mail_id = $1 AND state_version = $2
             AND execution_state IN ('enqueued', 'manual_reconcile')"#,
    )
    .bind(case.return_mail_id)
    .bind(action.state_version)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(crate::database::repository_error("结算退件 Take 动作状态已变化").into());
    }
    Ok(())
}

/// 领回之后的去向：计划补寄且仍满足补寄资格时授权新派发，否则替原权利人代管（权益保留）。
async fn dispose_taken(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    executable: Option<bool>,
    proof: &'static str,
    now: DateTime<Utc>,
) -> Result<SettlementReturnProgress, ReturnTransactionError> {
    let context = admission_facts(tx, case, executable, true).await?;
    let resend = match (case.plan, context.fulfillment.as_ref()) {
        (Some(SettlementReturnPlan::Resend), Some(fulfillment)) => {
            match context.admission.resend_allowed() {
                Ok(()) => Ok(fulfillment),
                Err(blocker) => Err(Some(blocker)),
            }
        }
        _ => Err(None),
    };
    let (next, details) = match resend {
        Ok(fulfillment) => {
            let dispatch_no = authorize_resend(tx, case, fulfillment, now).await?;
            let mut next =
                CaseTransition::from_case(case, SettlementReturnCaseStatus::ResendPending);
            next.plan = Some(SettlementReturnPlan::Resend);
            next.resend_dispatch_no = Some(dispatch_no);
            (next, json!({"next": "resend", "dispatch_no": dispatch_no, "proof": proof}))
        }
        Err(blocker) => {
            close_fulfillment(tx, case, context.fulfillment.as_ref(), HELD_FAILURE_CODE, now)
                .await?;
            let mut next = CaseTransition::from_case(case, SettlementReturnCaseStatus::Held);
            next.plan = Some(SettlementReturnPlan::Hold);
            (
                next,
                json!({
                    "next": "hold",
                    "resend_blocked": blocker.map(SettlementReturnBlocker::as_str),
                    "proof": proof,
                }),
            )
        }
    };
    let status = next.status;
    let version =
        transition(tx, case, &next, CaseEvent { kind: "take_proven", command: None, details }, now)
            .await?;
    Ok(SettlementReturnProgress {
        return_mail_id: case.return_mail_id,
        status,
        case_version: version,
        event_kind: "take_proven",
    })
}
