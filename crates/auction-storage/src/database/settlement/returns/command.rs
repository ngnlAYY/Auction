//! Admin 处理命令的单一 Serializable 事务：先按命令 ID 串行化并重放，再按既有业务锁序取锁、
//! 复核案件版本与领域准入，最后同事务写入案件投影、事件、命令账本与（领回时）持久 Take。命令
//! 只做本地写入；外部 Take 由 Worker 激活后经投递 Worker 执行（不变量 1、2、4）。

use auction_application::core::ApplicationError;
use auction_application::settlement::{
    SettlementReturnCommand, SettlementReturnCommandResult, SettlementReturnError,
    settlement_return_assets,
};
use auction_domain::{
    CommandId, SettlementReturnAction, SettlementReturnCaseStatus, SettlementReturnPlan,
    cash_return,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Postgres, Row};

use super::admission::{INBOX_FAILURE_CODE, admission_facts, obligation_kind};
use super::cases::{CaseEvent, CaseTransition, advance_case, append_event, load_case};
use super::errors::ReturnTransactionError;
use super::resend::{authorize_resend, close_fulfillment};
use super::take::{insert_action, take_command_id};
use crate::database::TrackedTransaction as Transaction;
use crate::database::mail::delivery::settlement_return_take_conflicts;
use crate::database::settlement::fulfillment::{
    lock_settlement_delivery_eligibility, recovery_allows_execution_now,
};
use crate::database::settlement::funding::lock_funded_delivery_account;

/// 执行一条 Admin 处理命令。
pub(super) async fn handle_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    command: &SettlementReturnCommand,
) -> Result<SettlementReturnCommandResult, ApplicationError> {
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Admin,
        crate::database::SERIALIZABLE_ATTEMPTS,
        ReturnTransactionError::retryable,
        || handle_once(pool, command),
    )
    .await
    .map_err(ReturnTransactionError::into_application)
}

async fn handle_once(
    pool: &crate::database::TrackedPool<Postgres>,
    command: &SettlementReturnCommand,
) -> Result<SettlementReturnCommandResult, ReturnTransactionError> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    crate::database::lock_command_id(&mut tx, command.command_id.as_uuid()).await?;
    // 同命令先重放，再检查当前版本：同 ID 同请求返回原结果，同 ID 不同请求冲突。
    if let Some(result) = replay(&mut tx, command).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let snapshot = load_case(&mut tx, command.return_mail_id, false)
        .await?
        .ok_or(SettlementReturnError::NotFound)?;
    let reclaims = matches!(
        command.action,
        SettlementReturnAction::ReclaimAndResend | SettlementReturnAction::ReclaimAndHold
    );
    // 锁序前缀：world（同邮件处置互斥）→ 资金 scope/账户 → 恢复记录与义务 → 案件 → 动作 → 履约。
    if reclaims {
        sqlx::query("SELECT id FROM public.worlds WHERE id = $1 FOR UPDATE")
            .bind(&snapshot.world_id)
            .execute(&mut *tx)
            .await?;
    }
    let kind = obligation_kind(&mut tx, snapshot.obligation_id).await?;
    let executable = match (snapshot.delivery_id, snapshot.obligation_id) {
        (Some(delivery_id), Some(obligation_id)) => {
            if kind.is_some_and(cash_return) {
                lock_funded_delivery_account(&mut tx, delivery_id).await?;
            }
            lock_settlement_delivery_eligibility(&mut tx, delivery_id).await?;
            Some(recovery_allows_execution_now(&mut tx, obligation_id).await?)
        }
        _ => None,
    };
    let case = load_case(&mut tx, command.return_mail_id, true)
        .await?
        .ok_or(SettlementReturnError::NotFound)?;
    if case.case_version != command.expected_version {
        return Err(SettlementReturnError::Stale.into());
    }
    let existing_action: Option<String> = sqlx::query_scalar(
        "SELECT execution_state FROM public.settlement_return_actions
         WHERE return_mail_id = $1 FOR UPDATE",
    )
    .bind(case.return_mail_id)
    .fetch_optional(&mut *tx)
    .await?;
    let context = admission_facts(&mut tx, &case, executable, true).await?;
    context.admission.check(command.action).map_err(SettlementReturnError::from)?;
    let now = command.requested_at;
    let actor = Some((command.command_id, command.actor_id.as_str()));
    let (next, kind_name, details) = match command.action {
        SettlementReturnAction::Acknowledge => (
            CaseTransition::from_case(&case, SettlementReturnCaseStatus::Reviewed),
            "acknowledged",
            json!({}),
        ),
        SettlementReturnAction::Recheck => (
            CaseTransition::from_case(&case, SettlementReturnCaseStatus::RecheckPending),
            "recheck_requested",
            json!({}),
        ),
        SettlementReturnAction::CloseInbox => {
            close_fulfillment(
                &mut tx,
                &case,
                context.fulfillment.as_ref(),
                INBOX_FAILURE_CODE,
                now,
            )
            .await?;
            (
                CaseTransition::from_case(&case, SettlementReturnCaseStatus::InboxClosed),
                "inbox_closed",
                json!({"asset_location": "returned_in_inbox"}),
            )
        }
        SettlementReturnAction::ReclaimAndResend | SettlementReturnAction::ReclaimAndHold => {
            if existing_action.is_some() {
                return Err(SettlementReturnError::ActionConflict.into());
            }
            if settlement_return_take_conflicts(
                &mut tx,
                &case.world_id,
                &case.external_mail_id,
                take_command_id(case.return_mail_id).as_uuid(),
            )
            .await?
            {
                return Err(SettlementReturnError::ActionConflict.into());
            }
            let assets = settlement_return_assets(&case.metadata)
                .ok_or(SettlementReturnError::AssetsMismatch)?;
            insert_action(&mut tx, &case, &assets, now).await?;
            let plan = if command.action == SettlementReturnAction::ReclaimAndResend {
                SettlementReturnPlan::Resend
            } else {
                SettlementReturnPlan::Hold
            };
            let mut next =
                CaseTransition::from_case(&case, SettlementReturnCaseStatus::TakePending);
            next.plan = Some(plan);
            (next, "take_requested", json!({"plan": plan.as_str()}))
        }
        SettlementReturnAction::ResendFromCustody => {
            let fulfillment =
                context.fulfillment.as_ref().ok_or(SettlementReturnError::ObligationIneligible)?;
            let dispatch_no = authorize_resend(&mut tx, &case, fulfillment, now).await?;
            let mut next =
                CaseTransition::from_case(&case, SettlementReturnCaseStatus::ResendPending);
            next.plan = Some(SettlementReturnPlan::Resend);
            next.resend_dispatch_no = Some(dispatch_no);
            (next, "resend_authorized", json!({"dispatch_no": dispatch_no}))
        }
    };
    let status = next.status;
    // 命令账本先于引用它的事件写入：事件以外键引用命令。
    let version = advance_case(&mut tx, &case, &next, now).await?;
    record_command(&mut tx, command, version, status, now).await?;
    append_event(
        &mut tx,
        case.return_mail_id,
        version,
        status,
        CaseEvent { kind: kind_name, command: actor, details },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(SettlementReturnCommandResult {
        command_id: command.command_id,
        return_mail_id: case.return_mail_id,
        action: command.action,
        case_version: version,
        status,
    })
}

/// 读取已提交的同 ID 命令：摘要一致返回原结果，不一致按冲突拒绝；损坏的账本行失败关闭。
async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    command: &SettlementReturnCommand,
) -> Result<Option<SettlementReturnCommandResult>, ReturnTransactionError> {
    let Some(row) = sqlx::query(
        r#"SELECT return_mail_id, action, request_digest, result_version, result_status
           FROM public.settlement_return_commands
           WHERE command_id = $1"#,
    )
    .bind(command.command_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(None);
    };
    let digest: Vec<u8> = row.try_get("request_digest")?;
    if digest.as_slice() != command.request_hash.as_bytes().as_slice() {
        return Err(SettlementReturnError::CommandConflict.into());
    }
    let action: String = row.try_get("action")?;
    let status: String = row.try_get("result_status")?;
    let (Some(action), Some(status)) = (
        SettlementReturnAction::from_name(&action),
        SettlementReturnCaseStatus::from_name(&status),
    ) else {
        return Err(SettlementReturnError::StoredResultCorrupt.into());
    };
    Ok(Some(SettlementReturnCommandResult {
        command_id: CommandId::from_uuid(command.command_id.as_uuid()),
        return_mail_id: row.try_get("return_mail_id")?,
        action,
        case_version: row.try_get("result_version")?,
        status,
    }))
}

async fn record_command(
    tx: &mut Transaction<'_, Postgres>,
    command: &SettlementReturnCommand,
    result_version: i64,
    result_status: SettlementReturnCaseStatus,
    now: DateTime<Utc>,
) -> Result<(), ReturnTransactionError> {
    sqlx::query(
        r#"INSERT INTO public.settlement_return_commands
               (command_id, return_mail_id, action, request_digest, actor_id, expected_version,
                result_version, result_status, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
    )
    .bind(command.command_id.as_uuid())
    .bind(command.return_mail_id)
    .bind(command.action.as_str())
    .bind(command.request_hash.as_bytes().as_slice())
    .bind(&command.actor_id)
    .bind(command.expected_version)
    .bind(result_version)
    .bind(result_status.as_str())
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
