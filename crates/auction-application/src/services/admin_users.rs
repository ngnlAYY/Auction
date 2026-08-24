//! Admin Users owner 的用户管理用例。
//!
//! 服务负责输入校验、请求摘要、命令重放、key 生成/封装和结果投影校验；用户行、改名历史、
//! 黑名单、审计和命令账本由 storage 在同一个事务中提交。它不执行 SQL，也不把明文 key
//! 放入持久化写入。

use std::sync::Arc;

use serde::Serialize;
use uuid::Uuid;

use crate::{
    AdminLoginKeyIssuer, AdminLoginKeyOpener, AdminRenameRevertCommand, AdminRenameRevertWrite,
    AdminUserCommandKind, AdminUserCommandResult, AdminUserCreateCommand, AdminUserCreateWrite,
    AdminUserError, AdminUserKeyMode, AdminUserKeyWrite, AdminUserListRequest,
    AdminUserMutationAction, AdminUserMutationCommand, AdminUserMutationResult,
    AdminUserMutationWrite, AdminUserPage, AdminUserPort, AdminUserStore, ApplicationError,
    ApplicationFuture, MailLoginKey, RequestHash,
};

/// Admin Users owner 用例服务。
pub struct AdminUserService<S, I, O> {
    store: Arc<S>,
    issuer: Arc<I>,
    opener: Arc<O>,
}

impl<S, I, O> AdminUserService<S, I, O>
where
    S: AdminUserStore,
    I: AdminLoginKeyIssuer,
    O: AdminLoginKeyOpener,
{
    /// 建立绑定 Users storage、key issuer 和历史 envelope opener 的服务。
    pub fn new(store: Arc<S>, issuer: Arc<I>, opener: Arc<O>) -> Self {
        Self { store, issuer, opener }
    }

    /// 读取有界用户管理列表。
    pub async fn list(
        &self,
        request: AdminUserListRequest,
    ) -> Result<AdminUserPage, ApplicationError> {
        if !request.validate() {
            return Err(AdminUserError::InvalidRequest.into());
        }
        let page = self.store.list(request.clone()).await?;
        if !page.validate(&request) {
            return Err(AdminUserError::StoredResultCorrupt.into());
        }
        Ok(page)
    }

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
        let response_envelope = wants_key.then(|| issue.envelope.clone());
        let result = self
            .store
            .create(AdminUserCreateWrite {
                command_id: command.command_id,
                request_hash,
                user_id,
                username: command.username,
                game_player_id: command.game_player_id,
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
            || result.user.id != user_id
            || result.user.state_version != 1
            || result.response_envelope.is_some() != wants_key
        {
            return Err(AdminUserError::StoredResultCorrupt.into());
        }
        Ok(AdminUserMutationResult {
            user: result.user,
            generated_key: wants_key.then_some(issue.key),
        })
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
        let public_key_hash = issue.as_ref().map(|value| value.key.public_key_hash());
        let expected_username = command.username.clone();
        let expected_game_player_id = command.game_player_id.clone();
        let key = issue.as_ref().map(|value| AdminUserKeyWrite {
            public_key_hash: public_key_hash.unwrap_or([0; 32]),
            envelope: value.envelope.clone(),
            response_envelope: Some(value.envelope.clone()),
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
                game_player_id: command.game_player_id,
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
            AdminUserMutationAction::SetGamePlayerId => {
                result.user.game_player_id.as_deref() == expected_game_player_id.as_deref()
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
        let generated_key = issue.map(|value| value.key);
        Ok(AdminUserMutationResult { user: result.user, generated_key })
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

impl<S, I, O> AdminUserPort for AdminUserService<S, I, O>
where
    S: AdminUserStore + 'static,
    I: AdminLoginKeyIssuer + 'static,
    O: AdminLoginKeyOpener + 'static,
{
    fn list(&self, request: AdminUserListRequest) -> ApplicationFuture<'_, AdminUserPage> {
        Box::pin(async move { AdminUserService::list(self, request).await })
    }

    fn create(
        &self,
        command: AdminUserCreateCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult> {
        Box::pin(async move { AdminUserService::create(self, command).await })
    }

    fn mutate(
        &self,
        command: AdminUserMutationCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult> {
        Box::pin(async move { AdminUserService::mutate(self, command).await })
    }

    fn revert_rename(
        &self,
        command: AdminRenameRevertCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult> {
        Box::pin(async move { AdminUserService::revert_rename(self, command).await })
    }
}

#[derive(Serialize)]
struct CreateDigest<'a> {
    username: &'a str,
    game_player_id: &'a str,
    key_mode: &'static str,
    key_hash: Option<[u8; 32]>,
}

#[derive(Serialize)]
struct MutationDigest<'a> {
    user_id: Uuid,
    expected_state_version: i64,
    action: &'static str,
    username: Option<&'a str>,
    game_player_id: Option<&'a str>,
    public_reason: Option<&'a str>,
}

#[derive(Serialize)]
struct RevertDigest {
    rename_id: Uuid,
    expected_state_version: i64,
}

fn create_hash(command: &AdminUserCreateCommand) -> Result<RequestHash, ApplicationError> {
    RequestHash::from_request(
        AdminUserCommandKind::Create.as_str(),
        Uuid::NAMESPACE_URL,
        &CreateDigest {
            username: &command.username,
            game_player_id: &command.game_player_id,
            key_mode: command.key_mode.as_str(),
            key_hash: command.manual_key.as_ref().map(MailLoginKey::public_key_hash),
        },
    )
    .map_err(ApplicationError::from)
}

fn mutation_hash(command: &AdminUserMutationCommand) -> Result<RequestHash, ApplicationError> {
    RequestHash::from_request(
        command.action.as_str(),
        command.user_id,
        &MutationDigest {
            user_id: command.user_id,
            expected_state_version: command.expected_state_version,
            action: command.action.as_str(),
            username: command.username.as_deref(),
            game_player_id: command.game_player_id.as_deref(),
            public_reason: command.public_reason.as_deref(),
        },
    )
    .map_err(ApplicationError::from)
}

fn revert_hash(command: &AdminRenameRevertCommand) -> Result<RequestHash, ApplicationError> {
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

fn validate_create(command: &AdminUserCreateCommand) -> Result<(), ApplicationError> {
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

fn validate_mutation(command: &AdminUserMutationCommand) -> Result<(), ApplicationError> {
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

fn validate_revert(command: &AdminRenameRevertCommand) -> Result<(), ApplicationError> {
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
