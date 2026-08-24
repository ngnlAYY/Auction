//! Admin 凭据管理用例。
//!
//! 本服务负责闭集校验、请求摘要、密封和结果投影检查；PostgreSQL storage 负责命令锁、
//! 版本 fence、凭据行、命令账本与审计的同事务提交。服务本身不读取配置、不执行 SQL，
//! 也不会把明文传入 storage。

use std::sync::Arc;

use serde::Serialize;
use uuid::Uuid;

use crate::{
    AdminCredential, AdminCredentialCommandKind, AdminCredentialCommandResult,
    AdminCredentialDeleteCommand, AdminCredentialDeleteResult, AdminCredentialDeleteWrite,
    AdminCredentialError, AdminCredentialImportCommand, AdminCredentialImportWrite,
    AdminCredentialListRequest, AdminCredentialMutationWrite, AdminCredentialPage,
    AdminCredentialPort, AdminCredentialRenameCommand, AdminCredentialSealer,
    AdminCredentialStatus, AdminCredentialStatusCommand, AdminCredentialStore, ApplicationError,
    ApplicationFuture, RequestHash, credential_scope, credential_secret_hash,
};

/// Admin 凭据管理服务。
pub struct AdminCredentialService<S, E> {
    store: Arc<S>,
    sealer: Arc<E>,
}

impl<S, E> AdminCredentialService<S, E>
where
    S: AdminCredentialStore,
    E: AdminCredentialSealer,
{
    /// 绑定凭据 storage 与独立的凭据数据密钥适配器。
    pub fn new(store: Arc<S>, sealer: Arc<E>) -> Self {
        Self { store, sealer }
    }

    /// 读取有界安全投影列表。
    pub async fn list(
        &self,
        request: AdminCredentialListRequest,
    ) -> Result<AdminCredentialPage, ApplicationError> {
        if !request.validate() {
            return Err(AdminCredentialError::InvalidRequest.into());
        }
        let page = self.store.list(request.clone()).await?;
        if !page.validate(&request) {
            return Err(AdminCredentialError::StoredResultCorrupt.into());
        }
        Ok(page)
    }

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

impl<S, E> AdminCredentialPort for AdminCredentialService<S, E>
where
    S: AdminCredentialStore + 'static,
    E: AdminCredentialSealer + 'static,
{
    fn list(
        &self,
        request: AdminCredentialListRequest,
    ) -> ApplicationFuture<'_, AdminCredentialPage> {
        Box::pin(async move { AdminCredentialService::list(self, request).await })
    }

    fn import(
        &self,
        command: AdminCredentialImportCommand,
    ) -> ApplicationFuture<'_, AdminCredential> {
        Box::pin(async move { AdminCredentialService::import(self, command).await })
    }

    fn rename(
        &self,
        command: AdminCredentialRenameCommand,
    ) -> ApplicationFuture<'_, AdminCredential> {
        Box::pin(async move { AdminCredentialService::rename(self, command).await })
    }

    fn set_status(
        &self,
        command: AdminCredentialStatusCommand,
    ) -> ApplicationFuture<'_, AdminCredential> {
        Box::pin(async move { AdminCredentialService::set_status(self, command).await })
    }

    fn delete(
        &self,
        command: AdminCredentialDeleteCommand,
    ) -> ApplicationFuture<'_, AdminCredentialDeleteResult> {
        Box::pin(async move { AdminCredentialService::delete(self, command).await })
    }
}

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

fn import_hash(command: &AdminCredentialImportCommand) -> Result<RequestHash, ApplicationError> {
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

fn rename_hash(command: &AdminCredentialRenameCommand) -> Result<RequestHash, ApplicationError> {
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

fn status_hash(command: &AdminCredentialStatusCommand) -> Result<RequestHash, ApplicationError> {
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

fn delete_hash(command: &AdminCredentialDeleteCommand) -> Result<RequestHash, ApplicationError> {
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

fn validate_import(command: &AdminCredentialImportCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || !valid_text(&command.label, 256)
        || !valid_text(&command.actor_id, 256)
        || command.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || command.now.timestamp_nanos_opt().is_none()
        || command.secret.as_bytes().is_empty()
    {
        return Err(AdminCredentialError::InvalidRequest.into());
    }
    Ok(())
}

fn validate_rename(command: &AdminCredentialRenameCommand) -> Result<(), ApplicationError> {
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

fn validate_status(command: &AdminCredentialStatusCommand) -> Result<(), ApplicationError> {
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

fn validate_delete(command: &AdminCredentialDeleteCommand) -> Result<(), ApplicationError> {
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

fn next_version(current: i64) -> Result<i64, ApplicationError> {
    current.checked_add(1).ok_or(AdminCredentialError::InvalidRequest.into())
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}
