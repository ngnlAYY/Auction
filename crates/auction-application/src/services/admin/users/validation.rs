//! Admin Users 命令字段与动作闭集校验。

use crate::{
    AdminRenameRevertCommand, AdminUserCreateCommand, AdminUserError, AdminUserKeyMode,
    AdminUserMutationAction, AdminUserMutationCommand, ApplicationError,
};

pub(crate) fn validate_create(command: &AdminUserCreateCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || !valid_text(&command.username, 256)
        || !valid_text(&command.game_player_id, 256)
        || !valid_text(&command.actor_id, 256)
        || command.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || command.now.timestamp_nanos_opt().is_none()
        || match command.key_mode {
            AdminUserKeyMode::Manual => command.manual_key.is_none(),
            AdminUserKeyMode::Generated => command.manual_key.is_some(),
        }
    {
        return Err(AdminUserError::InvalidRequest.into());
    }
    Ok(())
}

pub(crate) fn validate_mutation(
    command: &AdminUserMutationCommand,
) -> Result<(), ApplicationError> {
    let fields_valid = match command.action {
        AdminUserMutationAction::Rename => {
            command.username.as_deref().is_some_and(|value| valid_text(value, 256))
                && command.game_player_id.is_none()
                && command.public_reason.is_none()
        }
        AdminUserMutationAction::SetGamePlayerId => {
            command.game_player_id.as_deref().is_some_and(|value| valid_text(value, 256))
                && command.username.is_none()
                && command.public_reason.is_none()
        }
        AdminUserMutationAction::Blacklist => {
            command.public_reason.as_deref().is_some_and(|value| valid_text(value, 1024))
                && command.username.is_none()
                && command.game_player_id.is_none()
        }
        AdminUserMutationAction::RevokeKey
        | AdminUserMutationAction::ReissueKey
        | AdminUserMutationAction::Unblacklist => {
            command.username.is_none()
                && command.game_player_id.is_none()
                && command.public_reason.is_none()
        }
    };
    if command.command_id.is_nil()
        || command.user_id.is_nil()
        || command.expected_state_version <= 0
        || !valid_text(&command.actor_id, 256)
        || command.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || command.now.timestamp_nanos_opt().is_none()
        || !fields_valid
    {
        return Err(AdminUserError::InvalidRequest.into());
    }
    Ok(())
}

pub(crate) fn validate_revert(command: &AdminRenameRevertCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.rename_id.is_nil()
        || command.expected_state_version <= 0
        || !valid_text(&command.actor_id, 256)
        || command.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || command.now.timestamp_nanos_opt().is_none()
    {
        return Err(AdminUserError::InvalidRequest.into());
    }
    Ok(())
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}
