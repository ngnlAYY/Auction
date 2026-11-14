//! Admin 凭据命令的字段边界校验。

use auction_domain::valid_text;

use crate::errors::ApplicationError;
use crate::ports::admin::{
    AdminCredentialDeleteCommand, AdminCredentialError, AdminCredentialImportCommand,
    AdminCredentialRenameCommand, AdminCredentialStatusCommand,
};
use crate::services::admin::common::valid_command_envelope;

pub(super) fn validate_import(
    command: &AdminCredentialImportCommand,
) -> Result<(), ApplicationError> {
    if !valid_command_envelope(
        command.command_id,
        &command.actor_id,
        command.request_id.as_deref(),
        command.now,
    ) || !valid_text(&command.label, 256)
        || !crate::ports::admin::valid_credential_player_name(
            command.kind,
            command.player_name.as_deref(),
        )
    {
        return Err(AdminCredentialError::InvalidRequest.into());
    }
    Ok(())
}

pub(super) fn validate_rename(
    command: &AdminCredentialRenameCommand,
) -> Result<(), ApplicationError> {
    if !valid_command_envelope(
        command.command_id,
        &command.actor_id,
        command.request_id.as_deref(),
        command.now,
    ) || command.credential_id.is_nil()
        || command.expected_state_version <= 0
        || !valid_text(&command.label, 256)
    {
        return Err(AdminCredentialError::InvalidRequest.into());
    }
    Ok(())
}

pub(super) fn validate_status(
    command: &AdminCredentialStatusCommand,
) -> Result<(), ApplicationError> {
    if !valid_command_envelope(
        command.command_id,
        &command.actor_id,
        command.request_id.as_deref(),
        command.now,
    ) || command.credential_id.is_nil()
        || command.expected_state_version <= 0
        || !command.status.is_admin_selectable()
    {
        return Err(AdminCredentialError::InvalidRequest.into());
    }
    Ok(())
}

pub(super) fn validate_delete(
    command: &AdminCredentialDeleteCommand,
) -> Result<(), ApplicationError> {
    if !valid_command_envelope(
        command.command_id,
        &command.actor_id,
        command.request_id.as_deref(),
        command.now,
    ) || command.credential_id.is_nil()
        || command.expected_state_version <= 0
    {
        return Err(AdminCredentialError::InvalidRequest.into());
    }
    Ok(())
}

pub(super) fn next_version(current: i64) -> Result<i64, ApplicationError> {
    current.checked_add(1).ok_or_else(|| AdminCredentialError::InvalidRequest.into())
}
