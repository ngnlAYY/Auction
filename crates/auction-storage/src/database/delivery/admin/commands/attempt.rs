//! 单次 delivery Admin 命令事务：按固定顺序取锁、重放或识别冲突，交领域守卫判定后，同事务写入
//! 投递、结算履约、命令账本、审计与处置事实。

use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminGuardSnapshot,
    DeliveryAdminResult,
};
use sqlx::{Postgres, Row};

use super::super::eligibility::{
    key_recipient_renamed, permanently_retired, settlement_fulfillment_state,
};
use super::super::mapping::{legacy_result_payload, stored_snapshot};
use super::super::transaction::{DeliveryAdminTransactionError, PostgresDeliveryAdminTransaction};
use super::ledger::{committed_command, existing_command, lock_command};
use super::settlement::{
    admin_proof_digest, reset_settlement_fulfillment, update_settlement_fulfillment,
};
use super::transition::apply_delivery_action;
use crate::database::TrackedPool;
use crate::database::delivery::audit::{
    ADMIN_RECONCILE, ADMIN_RELEASE, ADMIN_RETRY_FAILED, DeliveryAudit,
    append as append_delivery_audit,
};
use crate::database::mail::delivery::disposition::mail_handed_over;
use crate::database::recovery::commands::reset_after_delivery_retry;
use crate::database::settlement::fulfillment::SettlementDeliveryEligibility;

pub(super) async fn execute_transaction(
    pool: &TrackedPool<Postgres>,
    command: &DeliveryAdminCommand,
) -> Result<DeliveryAdminResult, DeliveryAdminTransactionError> {
    let mut transaction = PostgresDeliveryAdminTransaction::begin(pool).await?;
    lock_command(&mut transaction, command.command_id).await?;
    if let Some(result) = existing_command(&mut transaction, command).await? {
        transaction.commit().await?;
        return Ok(result);
    }

    // 已提交的原命令先重放；新的人工命令不能改写已有资金绑定的出站责任。资金锁必须先于
    // delivery→fulfillment/recovery 读取，已绑定资金时立即失败关闭，避免反序取锁。
    // 该判定在领域守卫中重复一次，便于同一闭集阻断项覆盖所有调用路径。
    let funded = crate::database::settlement::funding::lock_funded_delivery_account(
        transaction.transaction_mut()?,
        command.delivery_id,
    )
    .await
    .map_err(DeliveryAdminTransactionError::Funding)?;
    let reactivates = matches!(
        command.action,
        DeliveryAdminAction::Release
            | DeliveryAdminAction::RetryFailed
            | DeliveryAdminAction::ReconcileRetry
    );
    // 会把投递改回 pending 的动作，先在资金锁前缀之后、投递行锁之前由结算 owner 按来源义务锁定
    // 恢复记录与义务，用唯一资格谓词复核：成交已撤销或已不是当前派发时不写任何状态，并优先于
    // 资金绑定给出明确业务错误（不变量 2、4、6）。
    let settlement_eligible = if reactivates {
        match crate::database::settlement::fulfillment::lock_settlement_delivery_eligibility(
            transaction.transaction_mut()?,
            command.delivery_id,
        )
        .await
        .map_err(DeliveryAdminTransactionError::Sqlx)?
        {
            SettlementDeliveryEligibility::NotSettlement => None,
            SettlementDeliveryEligibility::Eligible => Some(true),
            SettlementDeliveryEligibility::Ineligible => {
                return Err(DeliveryAdminError::SettlementIneligible.into());
            }
        }
    } else {
        None
    };
    if funded.is_some() {
        return Err(DeliveryAdminError::Invalid.into());
    }

    let retries_delivery = matches!(
        command.action,
        DeliveryAdminAction::RetryFailed | DeliveryAdminAction::ReconcileRetry
    );
    if retries_delivery {
        crate::database::recovery::commands::lock_recovery_delivery_retry_scope(
            transaction.transaction_mut()?,
            command.delivery_id,
        )
        .await
        .map_err(DeliveryAdminTransactionError::Sqlx)?;
    }

    // watch 重试不能倒取 lot 锁，更不能通过人工 release 恢复已经取消的领先周期。
    crate::database::lot_watch::lock_delivery_lot(
        transaction.transaction_mut()?,
        command.delivery_id,
    )
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    if retries_delivery {
        crate::database::recovery::commands::lock_recovery_delivery_retry_actions(
            transaction.transaction_mut()?,
            command.delivery_id,
        )
        .await
        .map_err(DeliveryAdminTransactionError::Sqlx)?;
    }
    let row = sqlx::query(
        r#"SELECT id, command_id, request_digest, activation_command_id,
                  activation_request_digest, origin_kind, origin_id, recipient, payload,
                  result_metadata, status,
                  lease_token,
                  lease_expires_at, fencing_version, attempt_count, claimed_at,
                  sending_at, completed_at, external_reference, failure_code,
                  activated_at, created_at, updated_at
           FROM public.deliveries
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(command.delivery_id)
    .fetch_optional(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?
    .ok_or(DeliveryAdminError::NotFound)?;
    // 已锁行的完整命令快照必须可读，避免以安全详情的部分字段改写不能重放的记录。
    let current = stored_snapshot(&row).ok_or(DeliveryAdminError::CommandSnapshotUnreadable)?;
    // 兼容滚动升级期间尚未取得 advisory lock 的旧实例：delivery 行锁等待结束后重新读取
    // 命令账本，必须先识别重放/冲突，再判断已经被前一命令推进的 fencing version。
    if let Some(result) = existing_command(&mut transaction, command).await? {
        transaction.commit().await?;
        return Ok(result);
    }
    // 旧投递已由追加修复事实永久退役；当前 fulfillment 指针复建后也不能把它当作普通孤儿重发。
    // 原成功命令已在上方重放；这里只拒绝新动作，因此不依赖用于放行的 trusted 证明匹配。
    let retired = permanently_retired(transaction.transaction_mut()?, &current)
        .await
        .map_err(DeliveryAdminTransactionError::Sqlx)?;
    // Worker 从 Sent 为当前人工核对或仍可更正的失败投递记录目击。重试可能重复寄出现金或装备，
    // 判失败会把已寄出误记为未发；动作准入在领域闭集中判定。
    let result_metadata: serde_json::Value =
        row.try_get("result_metadata").map_err(DeliveryAdminTransactionError::Sqlx)?;
    let sent_reference_sighted = result_metadata.get("sent_reference_sighting").is_some();
    let lot_eligible = if reactivates {
        Some(
            crate::database::lot_watch::delivery_eligible(
                transaction.transaction_mut()?,
                command.delivery_id,
            )
            .await
            .map_err(DeliveryAdminTransactionError::Sqlx)?,
        )
    } else {
        None
    };
    let mail_handed_over = reactivates
        && mail_handed_over(transaction.transaction_mut()?, command.delivery_id)
            .await
            .map_err(DeliveryAdminTransactionError::Sqlx)?;
    let payload: serde_json::Value =
        row.try_get("payload").map_err(DeliveryAdminTransactionError::Sqlx)?;
    let recipient_renamed = reactivates
        && key_recipient_renamed(transaction.transaction_mut()?, &current, &payload, true)
            .await
            .map_err(DeliveryAdminTransactionError::Sqlx)?;
    let (recovery_retry_eligible, recovery_retry) = if retries_delivery {
        let retry = reset_after_delivery_retry(transaction.transaction_mut()?, command.delivery_id)
            .await
            .map_err(DeliveryAdminTransactionError::Sqlx)?;
        (retry.eligible(), Some(retry))
    } else {
        (true, None)
    };
    let fulfillment_state = if command.action == DeliveryAdminAction::Release {
        None
    } else {
        settlement_fulfillment_state(transaction.transaction_mut()?, command.delivery_id, true)
            .await
            .map_err(DeliveryAdminTransactionError::Sqlx)?
    };
    let guard = DeliveryAdminGuardSnapshot {
        action: command.action,
        expected_fencing_version: command.expected_fencing_version,
        fencing_version: current.fencing_version,
        status: current.status.clone(),
        reason_code: current.reason_code.clone(),
        has_sending_fence: current.sending_at.is_some(),
        origin_kind: current.origin_kind.clone(),
        payload_operation: payload
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        sent_reference_sighted,
        funded_delivery: funded.is_some(),
        command_snapshot_readable: true,
        retired,
        lot_eligible,
        settlement_eligible,
        settlement_fulfillment_state: fulfillment_state,
        has_reconcile_reference: command.external_reference.is_some(),
        mail_handed_over,
        recipient_renamed,
        recovery_retry_eligible,
    };
    let decision = guard.evaluate();
    if let Some(blocker) = decision.blockers.first() {
        return Err(blocker.error().into());
    }
    let next_status = decision.transition.next_status;
    apply_delivery_action(&mut transaction, command, &current, &decision).await?;
    let fulfillment_state = guard.settlement_fulfillment_state.as_deref();
    if let Some(retry) = recovery_retry {
        reset_settlement_fulfillment(&mut transaction, command.delivery_id, fulfillment_state)
            .await?;
        // Recovery owner 在同一命令事务中恢复投影；失败则连 delivery 重试一并回滚。
        if !retry
            .apply(transaction.transaction_mut()?, command.delivery_id)
            .await
            .map_err(DeliveryAdminTransactionError::Sqlx)?
        {
            return Err(DeliveryAdminError::Stale.into());
        }
    } else if command.action == DeliveryAdminAction::ReconcileSucceeded {
        let external_reference =
            command.external_reference.as_deref().ok_or(DeliveryAdminError::Invalid)?;
        let proof_digest = admin_proof_digest(command.delivery_id, external_reference)?;
        update_settlement_fulfillment(
            &mut transaction,
            command.delivery_id,
            fulfillment_state,
            "satisfied",
            None,
            Some(external_reference),
            Some(proof_digest),
        )
        .await?;
    } else if command.action == DeliveryAdminAction::ReconcileFailed {
        update_settlement_fulfillment(
            &mut transaction,
            command.delivery_id,
            fulfillment_state,
            "failed",
            command.failure_code.as_deref(),
            None,
            None,
        )
        .await?;
    }

    let updated_row = sqlx::query(
        r#"SELECT id, command_id, request_digest, activation_command_id,
                  activation_request_digest, origin_kind, origin_id, recipient, payload,
                  result_metadata, status,
                  lease_token,
                  lease_expires_at, fencing_version, attempt_count, claimed_at,
                  sending_at, completed_at, external_reference, failure_code,
                  activated_at, created_at, updated_at
           FROM public.deliveries
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(command.delivery_id)
    .fetch_one(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    let (payload, snapshot) =
        legacy_result_payload(&updated_row).map_err(DeliveryAdminTransactionError::Business)?;
    if snapshot.status != next_status
        || snapshot.fencing_version <= command.expected_fencing_version
    {
        return Err(DeliveryAdminTransactionError::Business(
            DeliveryAdminError::StoredResultCorrupt,
        ));
    }
    let result = DeliveryAdminResult { command_id: command.command_id, snapshot };
    let inserted = sqlx::query(
        r#"INSERT INTO public.delivery_admin_commands
               (command_id, command_kind, request_digest, delivery_id, result_payload)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (command_id) DO NOTHING"#,
    )
    .bind(command.command_id)
    .bind(command.action.command_kind())
    .bind(command.request_hash.as_bytes().as_slice())
    .bind(command.delivery_id)
    .bind(payload)
    .execute(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    if inserted.rows_affected() != 1 {
        // 另一实例可能未使用同一 advisory lock。先回滚本事务中的 delivery/settlement
        // 变更，再从已提交账本恢复原结果；不能把相同命令误报为 conflict。
        transaction.rollback().await?;
        return committed_command(pool, command).await;
    }
    let (audit_action, admin_action) = match command.action {
        DeliveryAdminAction::Release => (ADMIN_RELEASE, "release"),
        DeliveryAdminAction::RetryFailed => (ADMIN_RETRY_FAILED, "retry_failed"),
        DeliveryAdminAction::ReconcileRetry => (ADMIN_RECONCILE, "reconcile_retry"),
        DeliveryAdminAction::ReconcileSucceeded => (ADMIN_RECONCILE, "reconcile_succeeded"),
        DeliveryAdminAction::ReconcileFailed => (ADMIN_RECONCILE, "reconcile_failed"),
    };
    append_delivery_audit(
        transaction.transaction_mut()?,
        DeliveryAudit {
            occurred_at: result.snapshot.updated_at,
            actor_type: "admin",
            actor_id: &command.actor_id,
            action: audit_action,
            delivery_id: command.delivery_id,
            request_id: command.request_id.as_deref(),
            source: "admin",
            metadata: serde_json::json!({
                "command_id": command.command_id,
                "admin_action": admin_action,
                "status": result.snapshot.status,
                "fencing_version": result.snapshot.fencing_version,
            }),
        },
    )
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    if command.action == DeliveryAdminAction::ReconcileFailed {
        // Admin 判失败本身就是该 fence 上的处置事实：读侧按它停止计失败，
        // 重试会推进 fence 从而自动重新计入。原命令账本与审计保持不变。
        crate::database::delivery::disposition::append(
            transaction.transaction_mut()?,
            crate::database::delivery::disposition::DeliveryDispositionFact {
                delivery_id: command.delivery_id,
                fencing_version: result.snapshot.fencing_version,
                kind: auction_domain::DeliveryDispositionKind::AdminFailed,
                source: auction_domain::DeliveryDispositionSource {
                    admin_command_id: Some(command.command_id),
                    ..Default::default()
                },
                actor_source: auction_domain::DeliveryDispositionActorSource::Admin,
                actor_id: &command.actor_id,
                reason: None,
                reason_code: command.failure_code.as_deref(),
                detail: Some(&serde_json::json!({
                    "admin_action": admin_action,
                    "status": result.snapshot.status,
                })),
                command_id: None,
                request_digest: None,
            },
        )
        .await
        .map_err(DeliveryAdminTransactionError::Sqlx)?;
    }
    transaction.commit().await?;
    Ok(result)
}
