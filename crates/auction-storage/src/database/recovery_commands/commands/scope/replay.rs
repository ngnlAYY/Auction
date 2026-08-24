//! Recovery 命令账本回放与 request fence。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(in crate::database::recovery_commands) async fn load_replayed_command(
    transaction: &mut Transaction<'_, Postgres>,
    command: &RecoveryConfirmationCommand,
) -> Result<Option<RecoveryConfirmationResult>, RecoveryCommandError> {
    let Some(row) = sqlx::query(
        r#"SELECT recovery_id, expected_state_version, action, request_digest,
                  result_code, result_payload
           FROM public.settlement_recovery_commands
           WHERE command_id = $1"#,
    )
    .bind(command.command_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    else {
        return Ok(None);
    };

    let stored_recovery_id: Uuid = row.try_get("recovery_id").map_err(map_sql_error)?;
    let stored_version: i64 = row.try_get("expected_state_version").map_err(map_sql_error)?;
    let stored_action: String = row.try_get("action").map_err(map_sql_error)?;
    let stored_digest: Vec<u8> = row.try_get("request_digest").map_err(map_sql_error)?;
    if stored_recovery_id != command.recovery_id.as_uuid()
        || stored_version != command.expected_state_version
        || stored_action != command.action.as_str()
        || stored_digest != command.request_hash.as_bytes()
    {
        return Err(RecoveryCommandError::CommandConflict);
    }

    let result_code: String = row.try_get("result_code").map_err(map_sql_error)?;
    if result_code != "ok" {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    let payload: Value = row.try_get("result_payload").map_err(map_sql_error)?;
    let result: RecoveryConfirmationResult =
        serde_json::from_value(payload).map_err(|_| RecoveryCommandError::InvalidStoredResult)?;
    result.validate()?;
    if result.recovery_id != command.recovery_id || result.action != command.action {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    if result.state_version <= command.expected_state_version {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    Ok(Some(result))
}
