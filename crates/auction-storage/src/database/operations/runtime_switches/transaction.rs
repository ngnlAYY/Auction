//! 命令锁 → 名称锁 → 最新版本 CAS；事实、接收结果和追加审计同事务提交。

use auction_application::runtime_switches::{
    RuntimeSwitchCommand, RuntimeSwitchError, RuntimeSwitchEvent, RuntimeSwitchOperation,
};
use sqlx::Row;
use uuid::Uuid;

use super::queries;
use crate::database::{DatabaseStore, operations::audit_health::AuditWriteResult as _};

#[derive(Debug, thiserror::Error)]
pub(super) enum WriteError {
    #[error("运行时开关 SQL 失败")]
    Sql(#[from] sqlx::Error),
    #[error(transparent)]
    Application(#[from] RuntimeSwitchError),
}

impl WriteError {
    pub(super) fn public(self) -> RuntimeSwitchError {
        match self {
            Self::Application(error) => error,
            Self::Sql(_) => RuntimeSwitchError::Unavailable,
        }
    }
}

pub(super) async fn write(
    store: &DatabaseStore,
    command: &RuntimeSwitchCommand,
) -> Result<RuntimeSwitchEvent, WriteError> {
    let digest = command.digest()?;
    // 显式 Read Committed，等待名称锁之后的语句取得最新已提交事实。
    let mut tx = store.pool().begin_with("BEGIN ISOLATION LEVEL READ COMMITTED").await?;
    crate::database::backend::lock_command_id(&mut tx, command.command_id).await?;
    if let Some(row) = sqlx::query("SELECT * FROM public.runtime_switch_events WHERE command_id=$1")
        .bind(command.command_id)
        .fetch_optional(&mut *tx)
        .await?
    {
        let event = command
            .replay(
                row.try_get("digest_version")?,
                &row.try_get::<Vec<u8>, _>("request_digest")?,
                queries::map(&row)?,
            )
            .map_err(WriteError::Application)?;
        let audit_id = row.try_get("audit_id")?;
        validate_replay_audit(&mut tx, command, audit_id).await?;
        validate_replay_provenance(&mut tx, command, &event).await?;
        return Ok(event);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text,0))")
        .bind(format!("runtime_switch:{}", command.name.label()))
        .execute(&mut *tx)
        .await?;
    let previous = sqlx::query(queries::LATEST)
        .bind(command.name.label())
        .fetch_optional(&mut *tx)
        .await?
        .as_ref()
        .map(queries::map)
        .transpose()?;
    let now = sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(&mut *tx).await?;
    let event = command.accept(previous.as_ref(), now)?;
    let audit_id = Uuid::new_v4();
    let (actor_type, source) = match command.entrypoint.as_str() {
        "admin" => ("admin", "admin"),
        "cli" => ("admin", "cli"),
        _ => ("system", "system"),
    };
    sqlx::query("INSERT INTO public.audit_records
        (id,occurred_at,actor_type,actor_id,action,entity_type,entity_id,result,source,metadata)
        VALUES($1,$2,$3,$4,$5,'runtime_switch',$6,'success',$7,$8)")
        .bind(audit_id).bind(now).bind(actor_type).bind(&command.actor_id)
        .bind(format!("runtime_switch.{}", command.operation.label())).bind(command.name.label()).bind(source)
        .bind(auction_application::core::cli_audit_metadata(command.operator_identity.as_ref(), serde_json::json!({"command_id": command.command_id, "entrypoint": command.entrypoint,
            "executor": if command.entrypoint == "cli" { "auction-cli" } else { command.entrypoint.as_str() },
            "previous": previous, "accepted": event})))
        .execute(&mut *tx).await.record_audit_failure()?;
    sqlx::query("INSERT INTO public.runtime_switch_events
        (command_id,digest_version,request_digest,switch_name,revision,expected_revision,operation,
         value,expires_at,expired_command_id,actor_id,entrypoint,reason,audit_id,result_payload,recorded_at)
        VALUES($1,1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
        .bind(command.command_id).bind(digest.as_slice()).bind(command.name.label()).bind(event.revision)
        .bind(command.expected_revision).bind(command.operation.label()).bind(&command.value).bind(command.expires_at)
        .bind(command.expired_command_id).bind(&command.actor_id).bind(&command.entrypoint).bind(&command.reason)
        .bind(audit_id).bind(serde_json::to_value(&event).map_err(invalid)?).bind(now)
        .execute(&mut *tx).await?;
    if let Err(error) = tx.commit().await {
        if crate::database::backend::observed_serialization_conflict(&error) {
            return Err(error.into());
        }
        return Err(RuntimeSwitchError::OutcomeUnknown.into());
    }
    Ok(event)
}
async fn validate_replay_audit(
    tx: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command: &RuntimeSwitchCommand,
    audit_id: Uuid,
) -> Result<(), WriteError> {
    let source = match command.entrypoint.as_str() {
        "admin" => "admin",
        "cli" => "cli",
        _ => "system",
    };
    let found: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM public.audit_records
            WHERE id = $1 AND action = $2 AND entity_type = 'runtime_switch'
              AND entity_id = $3 AND result = 'success' AND source = $4
              AND metadata->>'command_id' = $5
        )",
    )
    .bind(audit_id)
    .bind(format!("runtime_switch.{}", command.operation.label()))
    .bind(command.name.label())
    .bind(source)
    .bind(command.command_id.to_string())
    .fetch_one(&mut **tx)
    .await?;
    if !found {
        return Err(RuntimeSwitchError::Corrupt.into());
    }
    Ok(())
}

async fn validate_replay_provenance(
    tx: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command: &RuntimeSwitchCommand,
    event: &RuntimeSwitchEvent,
) -> Result<(), WriteError> {
    if command.operation != RuntimeSwitchOperation::Expire {
        return Ok(());
    }
    let predecessor_id = command.expired_command_id.ok_or(RuntimeSwitchError::Corrupt)?;
    let row = sqlx::query("SELECT * FROM public.runtime_switch_events WHERE command_id = $1")
        .bind(predecessor_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(RuntimeSwitchError::Corrupt)?;
    let predecessor = queries::map(&row)?;
    if predecessor.name != command.name
        || predecessor.revision != command.expected_revision
        || predecessor.operation != RuntimeSwitchOperation::Set
        || predecessor.expires_at.is_none_or(|deadline| deadline > event.recorded_at)
    {
        return Err(RuntimeSwitchError::Corrupt.into());
    }
    Ok(())
}

fn invalid<T>(_: T) -> RuntimeSwitchError {
    RuntimeSwitchError::Invalid
}
