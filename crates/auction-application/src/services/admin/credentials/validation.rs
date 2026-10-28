//! Admin 凭据命令的字段边界校验。

use auction_domain::valid_text;

use crate::{
    AdminCredentialDeleteCommand, AdminCredentialError, AdminCredentialImportCommand,
    AdminCredentialRenameCommand, AdminCredentialStatusCommand, ApplicationError,
};

pub(super) fn validate_import(
    command: &AdminCredentialImportCommand,
) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || !valid_text(&command.label, 256)
        || !valid_text(&command.actor_id, 256)
        || command.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || command.now.timestamp_nanos_opt().is_none()
        || !crate::valid_credential_player_name(command.kind, command.player_name.as_deref())
    {
        return Err(AdminCredentialError::InvalidRequest.into());
    }
    Ok(())
}

pub(super) fn validate_rename(
    command: &AdminCredentialRenameCommand,
) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.credential_id.is_nil()
        || command.expected_state_version <= 0
        || !valid_text(&command.label, 256)
        || !valid_text(&command.actor_id, 256)
        || command.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || command.now.timestamp_nanos_opt().is_none()
    {
        return Err(AdminCredentialError::InvalidRequest.into());
    }
    Ok(())
}

pub(super) fn validate_status(
    command: &AdminCredentialStatusCommand,
) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.credential_id.is_nil()
        || command.expected_state_version <= 0
        || !command.status.is_admin_selectable()
        || !valid_text(&command.actor_id, 256)
        || command.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || command.now.timestamp_nanos_opt().is_none()
    {
        return Err(AdminCredentialError::InvalidRequest.into());
    }
    Ok(())
}

pub(super) fn validate_delete(
    command: &AdminCredentialDeleteCommand,
) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.credential_id.is_nil()
        || command.expected_state_version <= 0
        || !valid_text(&command.actor_id, 256)
        || command.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || command.now.timestamp_nanos_opt().is_none()
    {
        return Err(AdminCredentialError::InvalidRequest.into());
    }
    Ok(())
}

pub(super) fn next_version(current: i64) -> Result<i64, ApplicationError> {
    current.checked_add(1).ok_or(AdminCredentialError::InvalidRequest.into())
}
