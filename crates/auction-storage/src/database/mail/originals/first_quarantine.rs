//! 整封首次隔离的写入：首次决定、持久隔离与同事务排入的系统重检（隔离回执永久可重放）。
//!
//! 保护不变量 3、4、8：隔离只写事实、不做任何资产外写；重检命令与工作随隔离同提交。

use auction_application::consignment::{
    DecideEnvelopeCommand, DepositActor, DepositOperation, EnvelopeDisposition, EnvelopeReceipt,
    QuarantineReference, RecheckReceipt,
};
use auction_application::consignment::{
    canonical_system_recheck_request_hash, system_command_id, system_recheck_business_key,
};
use auction_domain::consignment::deposit::QuarantineReason;
use auction_time::Timestamp;
use uuid::Uuid;

use super::disposition::insert_initial_decision;
use super::identity::QuarantineMetadata;
use super::observations::StoredObservation;
use super::quarantine::persist_quarantine;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::{DepositReceipt, DepositWriter};

/// 整封首次隔离：首次决定、隔离事实与同事务的系统重检一起写入，返回隔离回执。
///
/// 接管分支在锁内复判为“只隔离”时也走这里（`rts_intake`），零资产动作（不变量 4、6）。
pub(crate) async fn quarantine_envelope_in_tx(
    writer: &mut DepositWriter<'_, '_, '_>,
    command: &DecideEnvelopeCommand,
    stored: &StoredObservation,
    reason: QuarantineReason,
    now: Timestamp,
) -> Result<EnvelopeReceipt, WriteError> {
    let key = &command.mail_key;
    let command_id = writer.command_id();
    let decision_id = Uuid::new_v4();
    let quarantine_id = Uuid::new_v4();
    insert_initial_decision(
        writer.connection(),
        key,
        decision_id,
        stored.observation_id,
        command_id,
        "quarantined",
        Some(quarantine_id),
        now,
    )
    .await?;
    let metadata = QuarantineMetadata::new(&stored.mailbox_key);
    persist_quarantine(
        writer.connection(),
        key,
        quarantine_id,
        decision_id,
        stored.observation_id,
        reason,
        &metadata,
        now,
    )
    .await?;
    // 裁定：首次隔离在同一事务排一条系统重检；重检命中旧拍卖 owner 后由系统自动
    // 交接，其它解除仍由 Admin。命令与工作同提交，隔离回执永久可重放。
    enqueue_first_recheck(writer.connection(), quarantine_id, now).await?;
    let mut receipt =
        EnvelopeReceipt::new(command.command_id, EnvelopeDisposition::Quarantined, decision_id);
    receipt.quarantine = Some(QuarantineReference { quarantine_id, version: 1 });
    Ok(receipt)
}

/// 首次隔离同事务写入系统重检命令与 queued 工作。
async fn enqueue_first_recheck(
    connection: &mut sqlx::PgConnection,
    quarantine_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    let command_id = system_command_id(
        DepositOperation::RecheckQuarantine,
        &system_recheck_business_key(quarantine_id),
    );
    let request_hash = canonical_system_recheck_request_hash(quarantine_id, 1);
    let work_id = Uuid::new_v4();
    let receipt = RecheckReceipt::queued(command_id, quarantine_id, work_id);
    let body = receipt.body();
    sqlx::query(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, replay_key_id, listing_id, result_status, result_code,
             result_body, created_at, target_kind, target_id, target_bot_uid,
             target_world_id, target_external_mid, target_season_id, target_calendar_version)
         VALUES ($1, 2, 'system', $7, 'recheck_quarantine', $2,
                 $3, NULL, NULL, 202, 'ok', $4, $5, 'quarantine', $6, NULL, NULL, NULL,
                 NULL, NULL)",
    )
    .bind(command_id.as_uuid())
    .bind(quarantine_id.to_string())
    .bind(request_hash.as_bytes())
    .bind(&body)
    .bind(now)
    .bind(quarantine_id)
    .bind(DepositActor::DEPOSIT_SERVICE_REF)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.mail_quarantine_rechecks
            (id, quarantine_id, requested_version, command_id, state, available_at,
             lease_token, lease_until, fence, attempt_count, completed_at)
         VALUES ($1, $2, 1, $3, 'queued', $4, NULL, NULL, 0, 0, NULL)",
    )
    .bind(work_id)
    .bind(quarantine_id)
    .bind(command_id.as_uuid())
    .bind(now)
    .execute(&mut *connection)
    .await?;
    Ok(())
}
