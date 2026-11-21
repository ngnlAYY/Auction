//! Admin 再次领回（`retry_take`）：Take 结果未知、自动重排已用满，而案件最近一次事件正是当前
//! Take fence 上的 `assets_claimable` 观察（原完整资产仍在且可领取，上一次 Take 没有取走它）时，
//! 在命令事务内把同一 Take 投递重新排队、动作收回 enqueued，由投递 Worker 以新 fence 在原租约与
//! 发送围栏协议下再执行一次。执行前预检仍要求可领取且资产与冻结事实一致，已被领取的邮件不会被
//! 领取第二次；冻结凭据身份的前提（只配置 Cookie 时不领取）照常生效。
//!
//! 只认机器观察，不接受操作者声明；同邮件互斥在 world 锁下复核；重排不消耗也不重置自动重排
//! 预算，下一次结果未知仍只能由新的观察证明再次发起（不变量 1、2、4、6）。

use auction_application::settlement::{SettlementReturnCommand, SettlementReturnError};
use serde_json::{Value, json};
use sqlx::Postgres;

use super::cases::CaseRow;
use super::errors::ReturnTransactionError;
use super::take::{load_action, take_command_id};
use crate::database::TrackedTransaction as Transaction;
use crate::database::delivery::audit::{
    DeliveryAudit, SETTLEMENT_RETURN_RETAKE, append as append_delivery_audit,
};
use crate::database::mail::delivery::disposition::settlement_return_take_conflicts;

/// 重新排队本案的 Take 投递并把动作收回 enqueued，返回写进 `take_retry_requested` 事件的安全
/// 细节。调用方已按锁序持有 world → 资金 → 恢复与义务 → 案件 → 动作 → Take 投递的锁，并已在锁内
/// 判定准入（含机器观察证明）成立。
pub(super) async fn requeue_retried_take(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    command: &SettlementReturnCommand,
) -> Result<Value, ReturnTransactionError> {
    let action = load_action(tx, case.return_mail_id, true)
        .await?
        .ok_or(SettlementReturnError::ActionConflict)?;
    let Some(take_delivery) = action.delivery_id else {
        return Err(SettlementReturnError::ActionConflict.into());
    };
    if action.execution_state != "manual_reconcile" {
        return Err(SettlementReturnError::ActionConflict.into());
    }
    if settlement_return_take_conflicts(
        tx,
        &action.world_id,
        &action.source_external_mail_id,
        take_command_id(case.return_mail_id).as_uuid(),
    )
    .await?
    {
        return Err(SettlementReturnError::ActionConflict.into());
    }
    let (observed_version, fencing_version): (i64, i64) = sqlx::query_as(
        r#"SELECT case_version, (details ->> 'fencing_version')::bigint
           FROM public.settlement_return_events
           WHERE return_mail_id = $1
           ORDER BY case_version DESC
           LIMIT 1"#,
    )
    .bind(case.return_mail_id)
    .fetch_one(&mut **tx)
    .await?;
    // 只改观察所在 fence 上仍停在人工核对、没有远端引用的这条投递；任何一项变化都按状态冲突
    // 整体回滚。保留自动重排计数，下一次未知不会被当作“确定未外写”或重新获得自动重排。
    let requeued: Option<auction_time::Timestamp> = sqlx::query_scalar(
        r#"WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
           UPDATE public.deliveries
           SET status = 'pending', completed_at = NULL, failure_code = NULL, lease_token = NULL,
               lease_expires_at = NULL, claimed_at = NULL, sending_at = NULL,
               updated_at = (SELECT at FROM sql_clock),
               result_metadata = COALESCE(result_metadata, '{}'::jsonb)
                   || jsonb_build_object('retake_requested_at', (SELECT at FROM sql_clock),
                                         'retake_command_id', $3::uuid)
           WHERE id = $1 AND fencing_version = $2 AND status = 'manual_reconcile'
             AND origin_kind = 'settlement_return_action' AND payload_operation = 'take'
             AND external_reference IS NULL
           RETURNING updated_at"#,
    )
    .bind(take_delivery)
    .bind(fencing_version)
    .bind(command.command_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(requeued_at) = requeued else {
        return Err(SettlementReturnError::ActionConflict.into());
    };
    append_delivery_audit(
        tx,
        DeliveryAudit {
            occurred_at: requeued_at,
            actor_type: "admin",
            actor_id: &command.actor_id,
            action: SETTLEMENT_RETURN_RETAKE,
            delivery_id: take_delivery,
            request_id: None,
            source: "admin",
            metadata: json!({
                "status": "pending",
                "settlement_return": case.return_mail_id,
                "command_id": command.command_id.as_uuid(),
                "fencing_version": fencing_version,
                "observed_case_version": observed_version,
            }),
        },
    )
    .await?;
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
    .bind(case.return_mail_id)
    .bind(action.state_version)
    .bind(take_delivery)
    .bind(command.requested_at)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(SettlementReturnError::ActionConflict.into());
    }
    Ok(json!({
        "delivery_id": take_delivery,
        "fencing_version": fencing_version,
        "observed_case_version": observed_version,
    }))
}
