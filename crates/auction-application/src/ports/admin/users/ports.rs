//! Admin Users owner 的密钥、用户存储和 recovery 窄端口。

use uuid::Uuid;

use super::super::user_mail::{MailLoginKey, MailLoginKeyEnvelope};
use super::super::{ApplicationFuture, RequestHash};
use super::commands::{
    AdminRenameRevertCommand, AdminRenameRevertWrite, AdminUserCommandKind, AdminUserCommandResult,
    AdminUserCreateCommand, AdminUserCreateWrite, AdminUserKeyIssue, AdminUserMutationCommand,
    AdminUserMutationResult, AdminUserMutationWrite, RecoveryBuyerBlacklistCommand,
    RecoveryBuyerBlacklistResult,
};
use super::model::{AdminUserListRequest, AdminUserPage};

pub trait AdminLoginKeyIssuer: Send + Sync {
    fn generate(&self, user_id: Uuid) -> ApplicationFuture<'_, AdminUserKeyIssue>;
    fn seal(&self, user_id: Uuid, key: MailLoginKey) -> ApplicationFuture<'_, AdminUserKeyIssue>;
}

/// Admin 命令重放使用的历史密文解封端口。
pub trait AdminLoginKeyOpener: Send + Sync {
    fn open(
        &self,
        user_id: Uuid,
        envelope: MailLoginKeyEnvelope,
    ) -> ApplicationFuture<'_, MailLoginKey>;
}

/// Users owner 的 Admin 查询、幂等和原子写入端口。
pub trait AdminUserStore: Send + Sync {
    fn list(&self, request: AdminUserListRequest) -> ApplicationFuture<'_, AdminUserPage>;
    fn find_command(
        &self,
        command_id: Uuid,
        kind: AdminUserCommandKind,
        request_hash: RequestHash,
    ) -> ApplicationFuture<'_, Option<AdminUserCommandResult>>;
    fn create(&self, write: AdminUserCreateWrite) -> ApplicationFuture<'_, AdminUserCommandResult>;
    fn mutate(
        &self,
        write: AdminUserMutationWrite,
    ) -> ApplicationFuture<'_, AdminUserCommandResult>;
    fn revert_rename(
        &self,
        write: AdminRenameRevertWrite,
    ) -> ApplicationFuture<'_, AdminUserCommandResult>;
}

/// Users owner 的 Admin 查询与 mutation 端口。
pub trait AdminUserPort: Send + Sync {
    fn list(&self, request: AdminUserListRequest) -> ApplicationFuture<'_, AdminUserPage>;
    fn create(
        &self,
        command: AdminUserCreateCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult>;
    fn mutate(
        &self,
        command: AdminUserMutationCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult>;
    fn revert_rename(
        &self,
        command: AdminRenameRevertCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult>;
}

/// 该端口不把 recovery 状态写入 Users 表；它只负责用户黑名单、黑名单历史、用户命令账本和
/// 审计事实，完成后由 recovery owner 以独立 proof 推进 action。
pub trait RecoveryBuyerBlacklister: Send + Sync {
    fn ensure_buyer_blacklisted(
        &self,
        command: RecoveryBuyerBlacklistCommand,
    ) -> ApplicationFuture<'_, RecoveryBuyerBlacklistResult>;
}
