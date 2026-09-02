//! Admin Users 命令请求摘要。

use serde::Serialize;
use uuid::Uuid;

use crate::{
    AdminRenameRevertCommand, AdminUserCommandKind, AdminUserCreateCommand,
    AdminUserMutationCommand, ApplicationError, MailLoginKey, RequestHash,
};

#[derive(Serialize)]
struct CreateDigest<'a> {
    username: &'a str,
    forum_account_id: Option<&'a str>,
    key_mode: &'static str,
    key_hash: Option<[u8; 32]>,
}

#[derive(Serialize)]
struct MutationDigest<'a> {
    user_id: Uuid,
    expected_state_version: i64,
    action: &'static str,
    username: Option<&'a str>,
    forum_account_id: Option<&'a str>,
    public_reason: Option<&'a str>,
}

#[derive(Serialize)]
struct RevertDigest {
    rename_id: Uuid,
    expected_state_version: i64,
}

pub(crate) fn create_hash(
    command: &AdminUserCreateCommand,
) -> Result<RequestHash, ApplicationError> {
    RequestHash::from_request(
        AdminUserCommandKind::Create.as_str(),
        Uuid::NAMESPACE_URL,
        &CreateDigest {
            username: &command.username,
            forum_account_id: command.forum_account_id.as_deref(),
            key_mode: command.key_mode.as_str(),
            key_hash: command.manual_key.as_ref().map(MailLoginKey::public_key_hash),
        },
    )
    .map_err(ApplicationError::from)
}

pub(crate) fn mutation_hash(
    command: &AdminUserMutationCommand,
) -> Result<RequestHash, ApplicationError> {
    RequestHash::from_request(
        command.action.as_str(),
        command.user_id,
        &MutationDigest {
            user_id: command.user_id,
            expected_state_version: command.expected_state_version,
            action: command.action.as_str(),
            username: command.username.as_deref(),
            forum_account_id: command.forum_account_id.as_deref(),
            public_reason: command.public_reason.as_deref(),
        },
    )
    .map_err(ApplicationError::from)
}

pub(crate) fn revert_hash(
    command: &AdminRenameRevertCommand,
) -> Result<RequestHash, ApplicationError> {
    RequestHash::from_request(
        AdminUserCommandKind::RenameRevert.as_str(),
        command.rename_id,
        &RevertDigest {
            rename_id: command.rename_id,
            expected_state_version: command.expected_state_version,
        },
    )
    .map_err(ApplicationError::from)
}
