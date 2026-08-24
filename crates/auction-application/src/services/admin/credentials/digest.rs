//! Admin 凭据命令的稳定请求摘要。

use serde::Serialize;
use uuid::Uuid;

use crate::{
    AdminCredentialCommandKind, AdminCredentialDeleteCommand, AdminCredentialImportCommand,
    AdminCredentialRenameCommand, AdminCredentialStatusCommand, ApplicationError, RequestHash,
    credential_secret_hash,
};

#[derive(Serialize)]
struct ImportDigest<'a> {
    kind: &'static str,
    world: &'static str,
    label: &'a str,
    secret_hash: [u8; 32],
}

#[derive(Serialize)]
struct RenameDigest<'a> {
    credential_id: Uuid,
    expected_state_version: i64,
    label: &'a str,
}

#[derive(Serialize)]
struct StatusDigest {
    credential_id: Uuid,
    expected_state_version: i64,
    status: &'static str,
}

#[derive(Serialize)]
struct DeleteDigest {
    credential_id: Uuid,
    expected_state_version: i64,
}

pub(super) fn import_hash(
    command: &AdminCredentialImportCommand,
) -> Result<RequestHash, ApplicationError> {
    RequestHash::from_request(
        AdminCredentialCommandKind::Import.as_str(),
        Uuid::NAMESPACE_URL,
        &ImportDigest {
            kind: command.kind.as_str(),
            world: command.world.as_str(),
            label: &command.label,
            secret_hash: credential_secret_hash(command.secret.as_bytes()),
        },
    )
    .map_err(ApplicationError::from)
}

pub(super) fn rename_hash(
    command: &AdminCredentialRenameCommand,
) -> Result<RequestHash, ApplicationError> {
    RequestHash::from_request(
        AdminCredentialCommandKind::Rename.as_str(),
        command.credential_id,
        &RenameDigest {
            credential_id: command.credential_id,
            expected_state_version: command.expected_state_version,
            label: &command.label,
        },
    )
    .map_err(ApplicationError::from)
}

pub(super) fn status_hash(
    command: &AdminCredentialStatusCommand,
) -> Result<RequestHash, ApplicationError> {
    RequestHash::from_request(
        AdminCredentialCommandKind::SetStatus.as_str(),
        command.credential_id,
        &StatusDigest {
            credential_id: command.credential_id,
            expected_state_version: command.expected_state_version,
            status: command.status.as_str(),
        },
    )
    .map_err(ApplicationError::from)
}

pub(super) fn delete_hash(
    command: &AdminCredentialDeleteCommand,
) -> Result<RequestHash, ApplicationError> {
    RequestHash::from_request(
        AdminCredentialCommandKind::Delete.as_str(),
        command.credential_id,
        &DeleteDigest {
            credential_id: command.credential_id,
            expected_state_version: command.expected_state_version,
        },
    )
    .map_err(ApplicationError::from)
}
