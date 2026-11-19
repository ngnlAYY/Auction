//! 受证明恢复的状态迁移事务：同事务锁定观测、资金与动作，只重试明确冲突。
//!
//! 绑定原 Take 会话的恢复既服务入站邮件 Take，也服务结算退件 Take（`settlement_return_action`）：
//! 后者的投递改写与案件推进（领回记账或重新排队）在同一事务提交，任何一步不成立都整体回滚。

use auction_application::delivery::{
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliveryTakeExpectation,
};
use auction_time::Timestamp;
use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

use super::error::RecoveryError;
use super::recovery;
use crate::database::TrackedPool;
use crate::database::delivery::audit::{
    DeliveryAudit, RELEASE, SUCCEED, append as append_delivery_audit,
};
use crate::database::delivery::scheduler::{attempts, take_attempts};
use crate::database::settlement::funding;
use crate::database::settlement::returns::{
    SweptTakeOwner, TAKE_SWEEP_REASON, lock_swept_take, requeue_swept_take, settle_swept_take,
    swept_take_owner,
};

pub(super) enum RecoveryUpdate<'a> {
    Send(&'a str, &'a Value, i64, Option<&'a DeliverySendExpectation>),
    Action(&'a str, &'a Value, i64, &'a DeliveryTakeExpectation, &'a DeliverySendCredentialBinding),
    Requeue(i64, &'a str, i64, &'a DeliveryTakeExpectation, &'a DeliverySendCredentialBinding),
}

/// 只重试明确的序列化冲突/死锁；提交结果未知必须报错，不能计为恢复成功或重发。
pub(super) async fn persist_transition(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    update: RecoveryUpdate<'_>,
) -> Result<bool, auction_application::core::ApplicationError> {
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::MailboxSweep,
        crate::database::SERIALIZABLE_ATTEMPTS,
        RecoveryError::retryable,
        || persist_transition_once(pool, delivery_id, &update),
    )
    .await
    .map_err(|error| error.into_application_error("MoogleMail 恢复事务未确定提交，请核对原投递"))
}

async fn persist_transition_once(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    update: &RecoveryUpdate<'_>,
) -> Result<bool, RecoveryError> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    // 当前观测 generation→world→资金→recovery→credential→case→action→delivery；历史 generation
    // 只作身份。world、案件与动作只在结算退件 Take 时锁定。
    match update {
        RecoveryUpdate::Send(_, metadata, _, Some(expectation)) => {
            recovery::lock_observation(&mut tx, expectation, metadata).await?;
        }
        RecoveryUpdate::Action(_, _, _, expectation, observation)
        | RecoveryUpdate::Requeue(_, _, _, expectation, observation) => {
            recovery::lock_take_observation(&mut tx, expectation, observation).await?;
        }
        RecoveryUpdate::Send(_, _, _, None) => {}
    }
    let return_prefix = match update {
        RecoveryUpdate::Action(..) | RecoveryUpdate::Requeue(..) => {
            match swept_take_owner(&mut tx, delivery_id).await? {
                SweptTakeOwner::Other => None,
                SweptTakeOwner::Unprovable => {
                    tx.rollback().await?;
                    return Ok(false);
                }
                SweptTakeOwner::Return(prefix) => Some(prefix),
            }
        }
        RecoveryUpdate::Send(..) => None,
    };
    let account = funding::lock_funded_delivery_account(&mut tx, delivery_id).await?;
    let recovery_send = if matches!(update, RecoveryUpdate::Send(..)) {
        sqlx::query_scalar::<_, bool>(
            "SELECT origin_kind='settlement_recovery_action' FROM public.deliveries WHERE id=$1",
        )
        .bind(delivery_id)
        .fetch_one(&mut *tx)
        .await?
    } else {
        false
    };
    if recovery_send {
        crate::database::recovery::commands::lock_recovery_delivery_retry_scope(
            &mut tx,
            delivery_id,
        )
        .await?;
    }
    if let RecoveryUpdate::Send(_, _, _, Some(expectation)) = update
        && !attempts::lock_credential(&mut tx, expectation).await?
    {
        return Err(RecoveryError::Rejected);
    }
    if let RecoveryUpdate::Action(_, _, _, expectation, _)
    | RecoveryUpdate::Requeue(_, _, _, expectation, _) = update
        && !attempts::lock_credential_binding(
            &mut tx,
            &expectation.world_id,
            expectation.credential.as_ref(),
        )
        .await?
    {
        return Err(RecoveryError::Rejected);
    }
    if recovery_send {
        crate::database::recovery::commands::lock_recovery_delivery_retry_actions(
            &mut tx,
            delivery_id,
        )
        .await?;
    }
    // 结算退件只接受仍停在结果未知的案件；正常收口尚未接手的 take_pending 不由巡检改写。
    let returned = match return_prefix {
        Some(prefix) => match lock_swept_take(&mut tx, prefix).await? {
            Some(returned) => Some(returned),
            None => {
                tx.rollback().await?;
                return Ok(false);
            }
        },
        None => None,
    };
    let take_origin = if returned.is_some() { "settlement_return_action" } else { "mail_intake" };
    sqlx::query("SELECT id FROM public.deliveries WHERE id=$1 FOR UPDATE")
        .bind(delivery_id)
        .fetch_optional(&mut *tx)
        .await?;
    let taking_fencing_version = match update {
        RecoveryUpdate::Action(_, _, fencing_version, expectation, _)
        | RecoveryUpdate::Requeue(_, _, fencing_version, expectation, _) => {
            let Some((original_fencing, frozen)) =
                take_attempts::load_current(&mut tx, delivery_id, *fencing_version).await?
            else {
                tx.rollback().await?;
                return Ok(false);
            };
            if &frozen != *expectation {
                tx.rollback().await?;
                return Ok(false);
            }
            Some(original_fencing)
        }
        RecoveryUpdate::Send(..) => None,
    };
    let (origin_kind, origin_id, completed_at): (String, String, Timestamp) = match update {
        RecoveryUpdate::Requeue(retries, reason, fencing_version, _, _) => {
            let updated: Option<(String, Timestamp)> = sqlx::query_as(
                "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
                 UPDATE public.deliveries
                 SET status='pending', completed_at=NULL, failure_code=NULL, lease_token=NULL,
                     lease_expires_at=NULL, claimed_at=NULL, sending_at=NULL, updated_at=(SELECT at FROM sql_clock),
                     result_metadata=COALESCE(result_metadata, '{}'::jsonb)
                         || jsonb_build_object('sweep_retries',$2::bigint,
                             'sweep_requeued_at',(SELECT at FROM sql_clock),'sweep_requeue_reason',$3::text)
                 WHERE id=$1 AND status='manual_reconcile' AND origin_kind=$5
                   AND payload_operation='take' AND external_reference IS NULL
                   AND COALESCE((result_metadata->>'sweep_retries')::bigint,0)=$2-1
                   AND fencing_version=$4
                 RETURNING origin_id,updated_at",
            )
            .bind(delivery_id)
            .bind(retries)
            .bind(reason)
            .bind(fencing_version)
            .bind(take_origin)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = updated else {
                tx.rollback().await?;
                return Ok(false);
            };
            (take_origin.to_owned(), row.0, row.1)
        }
        RecoveryUpdate::Send(mid, metadata, fencing_version, expectation) => {
            let Some(row) = recovery::bind_send(
                &mut tx,
                delivery_id,
                *fencing_version,
                mid,
                metadata,
                *expectation,
            )
            .await?
            else {
                tx.rollback().await?;
                return Ok(false);
            };
            row
        }
        RecoveryUpdate::Action(mid, metadata, fencing_version, expectation, _) => {
            if *mid != expectation.external_mail_id
                || metadata.get("operation").and_then(Value::as_str) != Some("take")
                || metadata.get("proof").and_then(Value::as_str)
                    != Some("action_recovered_inbox_v1")
                || metadata.get("external_reference_proven").and_then(Value::as_bool) != Some(true)
                || metadata.get("recovered_mid").and_then(Value::as_str) != Some(*mid)
            {
                return Err(RecoveryError::Rejected);
            }
            // 结算退件只认 integrations 核对过原身份、原正文前缀与领取回执的完成观察；“附件为空”
            // 或邮件整体缺失都不能证明资产进入 Bot 账号，保持结果未知。预期资产与动作冻结事实的
            // 逐项比对由案件推进在同一事务内完成。
            if returned.is_some()
                && metadata.get("reason").and_then(Value::as_str) != Some(TAKE_SWEEP_REASON)
            {
                tx.rollback().await?;
                return Ok(false);
            }
            let world = &expectation.world_id;
            let reference = format!("mooglemail:{world}:take:{mid}");
            let updated: Option<(String, Timestamp)> = sqlx::query_as(
                "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
                 UPDATE public.deliveries
                 SET external_reference=$2,status='succeeded',failure_code=NULL,
                     completed_at=(SELECT at FROM sql_clock),updated_at=(SELECT at FROM sql_clock),
                     lease_token=NULL,lease_expires_at=NULL,
                     result_metadata=COALESCE(result_metadata,'{}'::jsonb)||$3::jsonb
                 WHERE id=$1 AND status='manual_reconcile' AND origin_kind=$7
                   AND payload_operation='take' AND external_reference IS NULL
                   AND sending_at IS NOT NULL AND COALESCE(payload_world,'main')=$4
                   AND payload->>'external_mail_id'=$5
                   AND fencing_version=$6
                   AND NOT EXISTS (
                       SELECT 1 FROM public.deliveries other WHERE other.external_reference=$2)
                 RETURNING origin_id,completed_at",
            )
            .bind(delivery_id)
            .bind(&reference)
            .bind(*metadata)
            .bind(world)
            .bind(*mid)
            .bind(fencing_version)
            .bind(take_origin)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = updated else {
                tx.rollback().await?;
                return Ok(false);
            };
            (take_origin.to_owned(), row.0, row.1)
        }
    };
    // 结算退件案件与投递改写同事务推进：完成证明按正常领回记账，仍在未领则回到 Take 待执行；
    // 锁内复核不成立时整体回滚，投递保持人工核对。
    let return_progress = match (returned.as_ref(), update) {
        (Some(returned), RecoveryUpdate::Action(..)) => {
            settle_swept_take(&mut tx, returned).await?
        }
        (Some(returned), RecoveryUpdate::Requeue(retries, reason, ..)) => {
            requeue_swept_take(&mut tx, returned, *retries, reason).await?
        }
        _ => None,
    };
    if returned.is_some() && return_progress.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }
    if matches!(update, RecoveryUpdate::Send(..))
        && let Some(account) = account
    {
        funding::finish_funded_delivery(
            &mut tx,
            &account,
            delivery_id,
            auction_application::settlement::FundingDeliveryOutcome::Sent,
        )
        .await?;
    }
    if origin_kind == "settlement" && matches!(update, RecoveryUpdate::Send(..)) {
        let obligation_id =
            Uuid::parse_str(&origin_id).map_err(|_: uuid::Error| RecoveryError::Rejected)?;
        recovery::resume_settlement_send(&mut tx, obligation_id, delivery_id, "manual_reconcile")
            .await?;
    }
    let resumed_action = if recovery_send {
        crate::database::recovery::commands::resume_after_delivery_proof(&mut tx, delivery_id)
            .await?
    } else {
        None
    };
    let requeued = matches!(update, RecoveryUpdate::Requeue(..));
    append_delivery_audit(
        &mut tx,
        DeliveryAudit {
            occurred_at: completed_at,
            actor_type: "worker",
            actor_id: "mailbox-maintenance",
            action: if requeued { RELEASE } else { SUCCEED },
            delivery_id,
            request_id: None,
            source: "worker",
            metadata: serde_json::json!({
                "status": if requeued { "pending" } else { "succeeded" },
                "recovered": !requeued,
                "mailbox_maintenance": true,
                "resumed_recovery_action": resumed_action,
                "fencing_version": match update {
                    RecoveryUpdate::Send(_, _, fencing_version, _)
                    | RecoveryUpdate::Action(_, _, fencing_version, _, _)
                    | RecoveryUpdate::Requeue(_, _, fencing_version, _, _) => *fencing_version,
                },
                "taking_fencing_version": taking_fencing_version,
                "observation_credential": match update {
                    RecoveryUpdate::Action(_, _, _, _, observation)
                    | RecoveryUpdate::Requeue(_, _, _, _, observation) => Some(observation),
                    RecoveryUpdate::Send(..) => None,
                },
                "sending_fencing_version": match update {
                    RecoveryUpdate::Send(_, metadata, _, Some(_)) => {
                        metadata.get("sending_fencing_version").and_then(Value::as_i64)
                    }
                    _ => None,
                },
                "recovered_mid": match update {
                    RecoveryUpdate::Send(mid, _, _, _) | RecoveryUpdate::Action(mid, _, _, _, _) => Some(*mid),
                    _ => None,
                },
                "settlement_return": return_progress.as_ref().map(|progress| serde_json::json!({
                    "return_mail_id": progress.return_mail_id,
                    "status": progress.status.as_str(),
                    "case_version": progress.case_version,
                })),
            }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}
