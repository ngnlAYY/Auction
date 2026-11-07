//! Users owner 的 MoogleMail 注册与换绑用例。
//! 该服务负责命令校验、幂等重放、登录 key 材料签发和 Users owner 调用。用户行、密文信封、
//! 命令账本与身份历史必须由 storage 在同一个事务中提交；服务本身不读取 SQL，也不把明文
//! key 放入返回结果或 delivery 命令。

use std::sync::Arc;

use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::ports::auction::ApplicationFuture;
use crate::ports::mail::{
    MailLoginKeyIssuer, MailProcessingError, MailUserCommandKind, MailUserPort,
    MailUserRebindCommand, MailUserRegisterCommand, MailUserRegisterWrite, MailUserRekeyCommand,
    MailUserRekeyWrite, MailUserResult, MailUserStore,
};

/// Users owner 的 MoogleMail 用例服务。
pub struct MailUserService<S, I> {
    store: Arc<S>,
    issuer: Arc<I>,
}

impl<S, I> MailUserService<S, I>
where
    S: MailUserStore,
    I: MailLoginKeyIssuer,
{
    pub fn new(store: Arc<S>, issuer: Arc<I>) -> Self {
        Self { store, issuer }
    }

    /// 首次创建用户，或按相同命令摘要恢复已有用户。
    pub async fn register_or_recover(
        &self,
        command: MailUserRegisterCommand,
    ) -> Result<MailUserResult, ApplicationError> {
        validate_register_command(&command)?;
        if let Some(result) = self
            .store
            .find_mail_command(
                command.command_id,
                MailUserCommandKind::Register,
                command.request_hash,
            )
            .await?
        {
            return result.validate(MailUserCommandKind::Register).map_err(ApplicationError::from);
        }

        // 已有用户恢复必须沿用实际 UUID；锁内身份 fence 防止查询后并发注册/改名错配 AAD。
        let user_id = self
            .store
            .find_registration_user_id(command.username.clone())
            .await?
            .unwrap_or_else(Uuid::new_v4);
        let issue = self.issuer.issue(user_id).await?;
        if user_id.is_nil()
            || issue.public_key_hash == [0; 32]
            || issue.envelope.ciphertext().len() < 16
        {
            return Err(MailProcessingError::InvalidLoginKeyEnvelope.into());
        }
        let result = self
            .store
            .register_or_recover(MailUserRegisterWrite {
                command,
                user_id,
                public_key_hash: issue.public_key_hash,
                envelope: issue.envelope,
            })
            .await?;
        result.validate(MailUserCommandKind::Register).map_err(ApplicationError::from)
    }

    /// 使用旧 key 摘要原子换绑可变 MM 玩家名称；命令重复时只重放已存结果。
    pub async fn rebind_username(
        &self,
        command: MailUserRebindCommand,
    ) -> Result<MailUserResult, ApplicationError> {
        validate_rebind_command(&command)?;
        if let Some(result) = self
            .store
            .find_mail_command(
                command.command_id,
                MailUserCommandKind::Rebind,
                command.request_hash,
            )
            .await?
        {
            return result.validate(MailUserCommandKind::Rebind).map_err(ApplicationError::from);
        }

        let result = self.store.rebind_username(command).await?;
        result.validate(MailUserCommandKind::Rebind).map_err(ApplicationError::from)
    }

    /// 为发件人账号轮换登录 key：新 key 与全部会话失效必须在同一个 Users 事务内提交。
    /// 账号不存在时不签发可用信封，只记录 `not_registered` 命令结果供回信说明。
    pub async fn rekey_login_key(
        &self,
        command: MailUserRekeyCommand,
    ) -> Result<MailUserResult, ApplicationError> {
        validate_rekey_command(&command)?;
        if let Some(result) = self
            .store
            .find_mail_command(command.command_id, MailUserCommandKind::Rekey, command.request_hash)
            .await?
        {
            return result.validate(MailUserCommandKind::Rekey).map_err(ApplicationError::from);
        }

        // 账号不存在时仍先选一个临时 UUID 以复用同一签发路径；storage 不会为此 UUID 落信封。
        let user_id = self
            .store
            .find_registration_user_id(command.username.clone())
            .await?
            .unwrap_or_else(Uuid::new_v4);
        let issue = self.issuer.issue(user_id).await?;
        if issue.public_key_hash == [0; 32] || issue.envelope.ciphertext().len() < 16 {
            return Err(MailProcessingError::InvalidLoginKeyEnvelope.into());
        }
        let result = self
            .store
            .rekey_login_key(MailUserRekeyWrite {
                command,
                user_id,
                public_key_hash: issue.public_key_hash,
                envelope: issue.envelope,
            })
            .await?;
        result.validate(MailUserCommandKind::Rekey).map_err(ApplicationError::from)
    }
}

impl<S, I> MailUserPort for MailUserService<S, I>
where
    S: MailUserStore + 'static,
    I: MailLoginKeyIssuer + 'static,
{
    fn register_or_recover(
        &self,
        command: MailUserRegisterCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        Box::pin(async move { MailUserService::register_or_recover(self, command).await })
    }

    fn rebind_username(
        &self,
        command: MailUserRebindCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        Box::pin(async move { MailUserService::rebind_username(self, command).await })
    }

    fn rekey_login_key(
        &self,
        command: MailUserRekeyCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        Box::pin(async move { MailUserService::rekey_login_key(self, command).await })
    }
}

fn validate_register_command(command: &MailUserRegisterCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.request_hash.as_bytes() == [0; 32]
        || !valid_identity(&command.username)
        || command.now.timestamp_nanos_opt().is_none()
    {
        return Err(MailProcessingError::InvalidUserCommand.into());
    }
    Ok(())
}

fn validate_rebind_command(command: &MailUserRebindCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.request_hash.as_bytes() == [0; 32]
        || command.old_key_hash == [0; 32]
        || !valid_identity(&command.next_username)
        || command.history_id.is_nil()
        || command.now.timestamp_nanos_opt().is_none()
    {
        return Err(MailProcessingError::InvalidUserCommand.into());
    }
    Ok(())
}

fn validate_rekey_command(command: &MailUserRekeyCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.request_hash.as_bytes() == [0; 32]
        || !valid_identity(&command.username)
        || command.now.timestamp_nanos_opt().is_none()
    {
        return Err(MailProcessingError::InvalidUserCommand.into());
    }
    Ok(())
}

fn valid_identity(value: &str) -> bool {
    auction_domain::valid_text(value, 256)
}
