//! Admin 凭据写入命令与安全结果重放。

use uuid::Uuid;

use super::{
    AdminCredentialService, delete_hash, import_hash, next_version, rename_hash, status_hash,
    validate_delete, validate_import, validate_rename, validate_status,
};
use crate::{
    AdminCredential, AdminCredentialCommandKind, AdminCredentialCommandResult,
    AdminCredentialDeleteCommand, AdminCredentialDeleteResult, AdminCredentialDeleteWrite,
    AdminCredentialError, AdminCredentialImportCommand, AdminCredentialImportWrite,
    AdminCredentialMutationWrite, AdminCredentialRenameCommand, AdminCredentialSealer,
    AdminCredentialStatus, AdminCredentialStatusCommand, AdminCredentialStore, ApplicationError,
    credential_scope,
};

impl<S, E> AdminCredentialService<S, E>
where
    S: AdminCredentialStore,
    E: AdminCredentialSealer,
{
    /// 导入并密封一条新的 mail/forum 凭据。
    pub async fn import(
        &self,
        command: AdminCredentialImportCommand,
    ) -> Result<AdminCredential, ApplicationError> {
        validate_import(&command)?;
        let request_hash = import_hash(&command)?;
        if let Some(result) = self
            .store
            .find_command(command.command_id, AdminCredentialCommandKind::Import, request_hash)
            .await?
        {
            return replay_credential(result);
        }

        let credential_id = Uuid::new_v4();
        let scope = credential_scope(command.kind, command.world);
        let envelope =
            self.sealer.seal(command.kind, credential_id, &scope, command.secret.as_bytes())?;
        let result = self
            .store
            .import(AdminCredentialImportWrite {
                command_id: command.command_id,
                request_hash,
                credential_id,
                kind: command.kind,
                world: command.world,
                label: command.label,
                envelope,
                actor_id: command.actor_id,
                request_id: command.request_id,
                now: command.now,
            })
            .await?;
        let credential = replay_credential(result)?;
        if credential.id != credential_id
            || credential.kind != command.kind
            || credential.world != command.world
            || credential.state_version != 1
            || credential.status != AdminCredentialStatus::Active
        {
            return Err(AdminCredentialError::StoredResultCorrupt.into());
        }
        Ok(credential)
    }

    /// 修改凭据标签并使用 state_version fence 防止覆盖并发变更。
    pub async fn rename(
        &self,
        command: AdminCredentialRenameCommand,
    ) -> Result<AdminCredential, ApplicationError> {
        validate_rename(&command)?;
        let request_hash = rename_hash(&command)?;
        if let Some(result) = self
            .store
            .find_command(command.command_id, AdminCredentialCommandKind::Rename, request_hash)
            .await?
        {
            return replay_credential(result);
        }
        let next_version = next_version(command.expected_state_version)?;
        let expected_label = command.label.clone();
        let result = self
            .store
            .mutate(AdminCredentialMutationWrite {
                command_id: command.command_id,
                request_hash,
                credential_id: command.credential_id,
                expected_state_version: command.expected_state_version,
                action: AdminCredentialCommandKind::Rename,
                label: Some(command.label),
                status: None,
                actor_id: command.actor_id,
                request_id: command.request_id,
                now: command.now,
            })
            .await?;
        let credential = replay_credential(result)?;
        if credential.id != command.credential_id
            || credential.state_version != next_version
            || credential.label != expected_label
        {
            return Err(AdminCredentialError::StoredResultCorrupt.into());
        }
        Ok(credential)
    }

    /// 修改凭据 active/disabled 状态；invalid 保留给 Worker 失败处理。
    pub async fn set_status(
        &self,
        command: AdminCredentialStatusCommand,
    ) -> Result<AdminCredential, ApplicationError> {
        validate_status(&command)?;
        let request_hash = status_hash(&command)?;
        if let Some(result) = self
            .store
            .find_command(command.command_id, AdminCredentialCommandKind::SetStatus, request_hash)
            .await?
        {
            return replay_credential(result);
        }
        let next_version = next_version(command.expected_state_version)?;
        let expected_status = command.status;
        let result = self
            .store
            .mutate(AdminCredentialMutationWrite {
                command_id: command.command_id,
                request_hash,
                credential_id: command.credential_id,
                expected_state_version: command.expected_state_version,
                action: AdminCredentialCommandKind::SetStatus,
                label: None,
                status: Some(command.status),
                actor_id: command.actor_id,
                request_id: command.request_id,
                now: command.now,
            })
            .await?;
        let credential = replay_credential(result)?;
        if credential.id != command.credential_id
            || credential.state_version != next_version
            || credential.status != expected_status
        {
            return Err(AdminCredentialError::StoredResultCorrupt.into());
        }
        Ok(credential)
    }

    /// 删除凭据并返回可安全重放的删除结果。
    pub async fn delete(
        &self,
        command: AdminCredentialDeleteCommand,
    ) -> Result<AdminCredentialDeleteResult, ApplicationError> {
        validate_delete(&command)?;
        let request_hash = delete_hash(&command)?;
        if let Some(result) = self
            .store
            .find_command(command.command_id, AdminCredentialCommandKind::Delete, request_hash)
            .await?
        {
            return replay_delete(result);
        }
        let next_version = next_version(command.expected_state_version)?;
        let result = self
            .store
            .delete(AdminCredentialDeleteWrite {
                command_id: command.command_id,
                request_hash,
                credential_id: command.credential_id,
                expected_state_version: command.expected_state_version,
                actor_id: command.actor_id,
                request_id: command.request_id,
                now: command.now,
            })
            .await?;
        let deleted = replay_delete(result)?;
        if deleted.credential_id != command.credential_id || deleted.state_version != next_version {
            return Err(AdminCredentialError::StoredResultCorrupt.into());
        }
        Ok(deleted)
    }
}

fn replay_credential(
    result: AdminCredentialCommandResult,
) -> Result<AdminCredential, ApplicationError> {
    if !result.validate() {
        return Err(AdminCredentialError::StoredResultCorrupt.into());
    }
    result.credential.ok_or(AdminCredentialError::StoredResultCorrupt.into())
}

fn replay_delete(
    result: AdminCredentialCommandResult,
) -> Result<AdminCredentialDeleteResult, ApplicationError> {
    if !result.validate() {
        return Err(AdminCredentialError::StoredResultCorrupt.into());
    }
    result.deleted.ok_or(AdminCredentialError::StoredResultCorrupt.into())
}
