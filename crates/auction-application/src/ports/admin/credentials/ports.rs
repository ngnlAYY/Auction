use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::errors::ApplicationError;

use super::{
    commands::{
        AdminCredentialCommandKind, AdminCredentialCommandResult, AdminCredentialDeleteCommand,
        AdminCredentialDeleteResult, AdminCredentialDeleteWrite, AdminCredentialImportCommand,
        AdminCredentialImportWrite, AdminCredentialMutationWrite, AdminCredentialRenameCommand,
        AdminCredentialStatusCommand,
    },
    value::{
        AdminCredential, AdminCredentialEnvelope, AdminCredentialHealthSnapshot,
        AdminCredentialKind, AdminCredentialListRequest, AdminCredentialPage,
        AdminCredentialRuntimeRecord, AdminCredentialWorld,
    },
};
use crate::ports::auction::ApplicationFuture;
use crate::ports::core::RequestHash;

/// Admin 凭据密封端口；mail/forum 使用不同的加密用途域。
pub trait AdminCredentialSealer: Send + Sync {
    fn seal(
        &self,
        kind: AdminCredentialKind,
        credential_id: Uuid,
        scope: &str,
        secret: &[u8],
    ) -> Result<AdminCredentialEnvelope, ApplicationError>;
}

/// 凭据查询、幂等和原子写入端口。
pub trait AdminCredentialStore: Send + Sync {
    fn list(
        &self,
        request: AdminCredentialListRequest,
    ) -> ApplicationFuture<'_, AdminCredentialPage>;
    fn find_command(
        &self,
        command_id: Uuid,
        kind: AdminCredentialCommandKind,
        request_hash: RequestHash,
    ) -> ApplicationFuture<'_, Option<AdminCredentialCommandResult>>;
    fn import(
        &self,
        write: AdminCredentialImportWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult>;
    fn mutate(
        &self,
        write: AdminCredentialMutationWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult>;
    fn delete(
        &self,
        write: AdminCredentialDeleteWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult>;
}

/// Worker active 凭据读取端口；查询只允许明确的 kind/world 组合。
pub trait AdminCredentialRuntimeReader: Send + Sync {
    /// 一次返回同 kind 下目标 world 与 Shared scope 的全部 active 凭据。
    /// 请求 Shared 时只返回 Shared；调用方无需再查 Shared 或合并去重，仍须拒绝多 active。
    fn list_active(
        &self,
        kind: AdminCredentialKind,
        world: AdminCredentialWorld,
    ) -> ApplicationFuture<'_, Vec<AdminCredentialRuntimeRecord>>;
}

/// Worker 凭据失效回写端口。
/// 远端明确返回登录失效时，Worker 只能把当前凭据标记为 invalid；该操作必须是幂等的，
/// 不能把远端写入结果重试成第二次写入，也不能通过 Admin 命令账本伪造一个用户命令。
pub trait AdminCredentialRuntimeWriter: Send + Sync {
    fn mark_invalid(&self, credential_id: Uuid, now: DateTime<Utc>) -> ApplicationFuture<'_, ()>;
}

/// Worker 持有数据库凭据运行时所有权的不透明 guard。
/// 具体互斥机制属于存储适配器；集成层在后续外部请求前检查所有权，不能读取数据库连接、
/// 凭据身份或任何敏感内容。检查失败或取消后永久失效，不自动重新获取所有权。
pub trait AdminCredentialRuntimeOwnership: Send + Sync {
    /// 检查原持锁会话；调用方必须施加总时限，超时丢弃 future 会永久撤销所有权。
    /// 成功只证明检查时仍可使用，不撤销已经到达远端的请求。
    fn check(&self) -> ApplicationFuture<'_, ()>;

    /// 同步读取已知失效状态；正常检查进行中仍有效，不能把它当作实时数据库探测。
    fn is_valid(&self) -> bool;

    /// 兼容旧 owner：没有持久 generation 和 sticky 撤销能力就不支持内部提交。
    fn mail_runtime_binding(&self) -> Option<crate::ports::settlement::MailRuntimeBinding> {
        None
    }

    /// 永久撤销该 owner 的全部后续操作；只有实现此能力才允许返回 mail binding。
    /// 默认 owner 不提供 binding，因此不会进入内部提交作用域。
    fn revoke_internal_use(&self) {}
}

/// Admin dashboard 的凭据聚合读取端口。
pub trait AdminCredentialHealthReader: Send + Sync {
    fn health(&self) -> ApplicationFuture<'_, AdminCredentialHealthSnapshot>;
}

/// 外部凭据解密端口；Worker 与 Admin 只读校验的明文只在集成适配器中短暂存在。
pub trait AdminCredentialOpener: Send + Sync {
    fn open(
        &self,
        kind: AdminCredentialKind,
        credential_id: Uuid,
        scope: &str,
        envelope: &AdminCredentialEnvelope,
    ) -> Result<Vec<u8>, ApplicationError>;
}

/// Admin 凭据用例端口。
pub trait AdminCredentialPort: Send + Sync {
    fn list(
        &self,
        request: AdminCredentialListRequest,
    ) -> ApplicationFuture<'_, AdminCredentialPage>;
    fn import(
        &self,
        command: AdminCredentialImportCommand,
    ) -> ApplicationFuture<'_, AdminCredential>;
    fn rename(
        &self,
        command: AdminCredentialRenameCommand,
    ) -> ApplicationFuture<'_, AdminCredential>;
    fn set_status(
        &self,
        command: AdminCredentialStatusCommand,
    ) -> ApplicationFuture<'_, AdminCredential>;
    fn delete(
        &self,
        command: AdminCredentialDeleteCommand,
    ) -> ApplicationFuture<'_, AdminCredentialDeleteResult>;
}

/// 建立凭据导入命令使用的请求摘要；明文只进入 SHA-256，不进入账本。
pub fn credential_secret_hash(secret: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(secret);
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}

/// 将 kind/world 转换为内部 scope，并拒绝不属于凭据闭集的值。
pub fn credential_scope(kind: AdminCredentialKind, world: AdminCredentialWorld) -> String {
    world.scope(kind)
}
