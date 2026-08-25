//! Users/MoogleMail 注册、换绑和 key 生命周期窄端口。

use uuid::Uuid;

use super::super::{ApplicationFuture, MailUserCommandKind, MailUserResult, RequestHash};
use super::commands::{MailUserRebindCommand, MailUserRegisterWrite};
use super::security::{MailLoginKey, MailLoginKeyEnvelope, MailLoginKeyIssue};

/// Users owner 的注册、换绑和幂等读取端口。
pub trait MailUserStore: Send + Sync {
    fn find_mail_command(
        &self,
        command_id: auction_domain::CommandId,
        kind: MailUserCommandKind,
        request_hash: RequestHash,
    ) -> ApplicationFuture<'_, Option<MailUserResult>>;

    fn register_or_recover(
        &self,
        write: MailUserRegisterWrite,
    ) -> ApplicationFuture<'_, MailUserResult>;

    fn rebind_game_identity(
        &self,
        command: MailUserRebindCommand,
    ) -> ApplicationFuture<'_, MailUserResult>;
}

/// 读取登录 key 密文信封的窄端口；解密能力不属于 storage owner。
pub trait MailLoginKeyEnvelopeReader: Send + Sync {
    fn load_login_key_envelope(&self, user_id: Uuid)
    -> ApplicationFuture<'_, MailLoginKeyEnvelope>;
}

/// Worker 专用登录 key 签发端口，只返回摘要和密文信封。
pub trait MailLoginKeyIssuer: Send + Sync {
    fn issue(&self, user_id: Uuid) -> ApplicationFuture<'_, MailLoginKeyIssue>;
}

/// Delivery transport 专用的短生命周期明文读取端口。
pub trait MailLoginKeyReader: Send + Sync {
    fn read(&self, user_id: Uuid) -> ApplicationFuture<'_, MailLoginKey>;
}

/// Users owner 用例端口。
pub trait MailUserPort: Send + Sync {
    fn register_or_recover(
        &self,
        command: super::commands::MailUserRegisterCommand,
    ) -> ApplicationFuture<'_, MailUserResult>;

    fn rebind_game_identity(
        &self,
        command: MailUserRebindCommand,
    ) -> ApplicationFuture<'_, MailUserResult>;
}
