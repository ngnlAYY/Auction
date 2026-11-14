//! Admin Users 命令字段与动作闭集校验。

use auction_domain::valid_text;

use crate::errors::ApplicationError;
use crate::ports::admin::{
    AdminRenameRevertCommand, AdminUserCreateCommand, AdminUserError, AdminUserKeyMode,
    AdminUserMutationAction, AdminUserMutationCommand,
};
use crate::services::admin::envelope::valid_command_envelope;

pub(crate) fn validate_create(command: &AdminUserCreateCommand) -> Result<(), ApplicationError> {
    if !valid_command_envelope(
        command.command_id,
        &command.actor_id,
        command.request_id.as_deref(),
        command.now,
    ) || !valid_text(&command.username, 256)
        || command.forum_account_id.as_deref().is_some_and(|value| !valid_forum_account_id(value))
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
                && command.forum_account_id.is_none()
                && command.public_reason.is_none()
        }
        AdminUserMutationAction::SetForumAccountId => {
            command.forum_account_id.as_deref().is_some_and(valid_forum_account_id)
                && command.username.is_none()
                && command.public_reason.is_none()
        }
        AdminUserMutationAction::Blacklist => {
            command.public_reason.as_deref().is_some_and(|value| valid_text(value, 1024))
                && command.username.is_none()
                && command.forum_account_id.is_none()
        }
        AdminUserMutationAction::RevokeKey
        | AdminUserMutationAction::ReissueKey
        | AdminUserMutationAction::Unblacklist => {
            command.username.is_none()
                && command.forum_account_id.is_none()
                && command.public_reason.is_none()
        }
    };
    if !valid_command_envelope(
        command.command_id,
        &command.actor_id,
        command.request_id.as_deref(),
        command.now,
    ) || command.user_id.is_nil()
        || command.expected_state_version <= 0
        || !fields_valid
    {
        return Err(AdminUserError::InvalidRequest.into());
    }
    Ok(())
}

pub(crate) fn validate_revert(command: &AdminRenameRevertCommand) -> Result<(), ApplicationError> {
    if !valid_command_envelope(
        command.command_id,
        &command.actor_id,
        command.request_id.as_deref(),
        command.now,
    ) || command.rename_id.is_nil()
        || command.expected_state_version <= 0
    {
        return Err(AdminUserError::InvalidRequest.into());
    }
    Ok(())
}

fn valid_forum_account_id(value: &str) -> bool {
    auction_domain::valid_positive_decimal(value, 32)
}
