//! Admin Users 创建、修改和改名回滚命令用例。

use uuid::Uuid;

use super::{
    AdminUserService, create_hash, mutation_hash, revert_hash, validate_create, validate_mutation,
    validate_revert,
};
use crate::{
    AdminLoginKeyIssuer, AdminLoginKeyOpener, AdminRenameRevertCommand, AdminRenameRevertWrite,
    AdminUserCommandKind, AdminUserCommandResult, AdminUserCreateCommand, AdminUserCreateWrite,
    AdminUserError, AdminUserKeyMode, AdminUserKeyWrite, AdminUserMutationAction,
    AdminUserMutationCommand, AdminUserMutationResult, AdminUserMutationWrite, AdminUserStore,
    ApplicationError,
};

impl<S, I, O> AdminUserService<S, I, O>
where
    S: AdminUserStore,
    I: AdminLoginKeyIssuer,
    O: AdminLoginKeyOpener,
{
    /// 创建用户并按旧契约决定是否返回一次性生成 key。
    pub async fn create(
        &self,
        mut command: AdminUserCreateCommand,
    ) -> Result<AdminUserMutationResult, ApplicationError> {
        validate_create(&command)?;
        let request_hash = create_hash(&command)?;
        let wants_key = command.key_mode == AdminUserKeyMode::Generated;
        if let Some(result) = self
            .store
            .find_command(command.command_id, AdminUserCommandKind::Create, request_hash)
            .await?
        {
            return self.replay(result, wants_key).await;
        }

        let user_id = Uuid::new_v4();
        let issue = match command.key_mode {
            AdminUserKeyMode::Generated => self.issuer.generate(user_id).await?,
            AdminUserKeyMode::Manual => {
                let key = command.manual_key.take().ok_or(AdminUserError::InvalidRequest)?;
                self.issuer.seal(user_id, key).await?
            }
        };
        let public_key_hash = issue.key.public_key_hash();
        let expected_username = command.username.clone();
        let expected_forum_account_id = command.forum_account_id.clone();
        let response_envelope = wants_key.then(|| issue.envelope.clone());
        let result = self
            .store
            .create(AdminUserCreateWrite {
                command_id: command.command_id,
                request_hash,
                user_id,
                username: command.username,
                forum_account_id: command.forum_account_id,
                key: AdminUserKeyWrite {
                    public_key_hash,
                    envelope: issue.envelope,
                    response_envelope,
                },
                actor_id: command.actor_id,
                request_id: command.request_id,
                now: command.now,
            })
            .await?;
        if !result.validate()
            || result.user.username != expected_username
            || result.user.forum_account_id != expected_forum_account_id
            || result.user.state_version != 1
            || result.response_envelope.is_some() != wants_key
        {
            return Err(AdminUserError::StoredResultCorrupt.into());
        }
        // 事务内可能重放另一并发请求的结果；用户 ID 和返回 key 必须取自已提交结果。
        self.replay(result, wants_key).await
    }

    /// 执行带 state_version fence 的用户修改。
    pub async fn mutate(
        &self,
        command: AdminUserMutationCommand,
    ) -> Result<AdminUserMutationResult, ApplicationError> {
        validate_mutation(&command)?;
        let request_hash = mutation_hash(&command)?;
        let kind = command.action.kind();
        let wants_key = command.action == AdminUserMutationAction::ReissueKey;
        if let Some(result) =
            self.store.find_command(command.command_id, kind, request_hash).await?
        {
            return self.replay(result, wants_key).await;
        }

        let issue =
            if wants_key { Some(self.issuer.generate(command.user_id).await?) } else { None };
        let expected_username = command.username.clone();
        let expected_forum_account_id = command.forum_account_id.clone();
        let key = issue.map(|value| AdminUserKeyWrite {
            public_key_hash: value.key.public_key_hash(),
            response_envelope: Some(value.envelope.clone()),
            envelope: value.envelope,
        });
        let result = self
            .store
            .mutate(AdminUserMutationWrite {
                command_id: command.command_id,
                request_hash,
                user_id: command.user_id,
                expected_state_version: command.expected_state_version,
                action: command.action,
                username: command.username,
                forum_account_id: command.forum_account_id,
                public_reason: command.public_reason,
                key,
                actor_id: command.actor_id,
                request_id: command.request_id,
                now: command.now,
            })
            .await?;
        let next_version =
            command.expected_state_version.checked_add(1).ok_or(AdminUserError::InvalidRequest)?;
        let action_result_valid = match command.action {
            AdminUserMutationAction::Rename => {
                result.user.username == expected_username.as_deref().unwrap_or_default()
            }
            AdminUserMutationAction::SetForumAccountId => {
                result.user.forum_account_id.as_deref() == expected_forum_account_id.as_deref()
            }
            AdminUserMutationAction::RevokeKey => {
                result.user.key_status == crate::AdminUserKeyStatus::Revoked
            }
            AdminUserMutationAction::ReissueKey => {
                result.user.key_status == crate::AdminUserKeyStatus::Active
            }
            AdminUserMutationAction::Blacklist => {
                result.user.status == crate::AdminUserStatus::Blacklisted
            }
            AdminUserMutationAction::Unblacklist => {
                result.user.status == crate::AdminUserStatus::Active
            }
        };
        if !result.validate()
            || result.user.id != command.user_id
            || result.user.state_version != next_version
            || result.response_envelope.is_some() != wants_key
            || !action_result_valid
        {
            return Err(AdminUserError::StoredResultCorrupt.into());
        }
        // 并发重放可能返回另一请求密封的 key，不能返回本次尚未入库的候选 key。
        self.replay(result, wants_key).await
    }

    /// 回滚一条未被回滚且仍满足版本 fence 的改名事实。
    pub async fn revert_rename(
        &self,
        command: AdminRenameRevertCommand,
    ) -> Result<AdminUserMutationResult, ApplicationError> {
        validate_revert(&command)?;
        let request_hash = revert_hash(&command)?;
        if let Some(result) = self
            .store
            .find_command(command.command_id, AdminUserCommandKind::RenameRevert, request_hash)
            .await?
        {
            return self.replay(result, false).await;
        }
        let result = self
            .store
            .revert_rename(AdminRenameRevertWrite {
                command_id: command.command_id,
                request_hash,
                rename_id: command.rename_id,
                expected_state_version: command.expected_state_version,
                actor_id: command.actor_id,
                request_id: command.request_id,
                now: command.now,
            })
            .await?;
        let next_version =
            command.expected_state_version.checked_add(1).ok_or(AdminUserError::InvalidRequest)?;
        if !result.validate()
            || result.user.state_version != next_version
            || result.response_envelope.is_some()
        {
            return Err(AdminUserError::StoredResultCorrupt.into());
        }
        Ok(AdminUserMutationResult { user: result.user, generated_key: None })
    }

    async fn replay(
        &self,
        result: AdminUserCommandResult,
        wants_key: bool,
    ) -> Result<AdminUserMutationResult, ApplicationError> {
        if !result.validate() || result.response_envelope.is_some() != wants_key {
            return Err(AdminUserError::StoredResultCorrupt.into());
        }
        let generated_key = if wants_key {
            let envelope = result.response_envelope.ok_or(AdminUserError::StoredResultCorrupt)?;
            Some(
                self.opener
                    .open(result.user.id, envelope)
                    .await
                    .map_err(|_| AdminUserError::KeyUnavailable)?,
            )
        } else {
            None
        };
        Ok(AdminUserMutationResult { user: result.user, generated_key })
    }
}
