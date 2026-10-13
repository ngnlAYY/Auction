//! 已保存外部 Cookie 的只读认证探测契约；不改变凭据状态或命令账本。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
    AdminCredential, AdminCredentialEnvelope, AdminCredentialKind, AdminCredentialWorld,
    ApplicationFuture,
};

/// 认证探测的闭集结果；网络、限流和协议变化不能伪装成凭据失效。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminCredentialCheckStatus {
    Valid,
    Invalid,
    Inconclusive,
}

impl AdminCredentialCheckStatus {
    /// 返回稳定的安全状态码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::Inconclusive => "inconclusive",
        }
    }
}

/// 一个明确世界/论坛登录范围的观测，不包含账号、余额、Cookie 或响应正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCredentialCheckObservation {
    pub world: AdminCredentialWorld,
    pub status: AdminCredentialCheckStatus,
}

/// 绑定凭据版本的瞬时结果，不作为持久化健康状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCredentialCheckResult {
    pub credential_id: Uuid,
    pub state_version: i64,
    pub checked_at: DateTime<Utc>,
    pub checks: Vec<AdminCredentialCheckObservation>,
}

/// 只供认证探测适配器解密的记录；不允许进入 HTTP DTO。
pub struct AdminCredentialCheckRecord {
    pub credential: AdminCredential,
    pub envelope: AdminCredentialEnvelope,
}

/// 按精确 ID 读取凭据；包含 disabled/invalid，但不能隐式启用它们。
pub trait AdminCredentialCheckStore: Send + Sync {
    fn load_for_check(
        &self,
        credential_id: Uuid,
    ) -> ApplicationFuture<'_, AdminCredentialCheckRecord>;
}

/// 外部只读认证探测；只接受数据库信封，禁止浏览器指定 URL 或提供任意 Cookie。
pub trait AdminCredentialVerifier: Send + Sync {
    fn verify(
        &self,
        record: AdminCredentialCheckRecord,
    ) -> ApplicationFuture<'_, Vec<AdminCredentialCheckObservation>>;
}

/// Admin 手动校验端口；只读观测不使用会冻结旧结果的幂等命令账本。
pub trait AdminCredentialCheckPort: Send + Sync {
    fn check(
        &self,
        credential_id: Uuid,
        expected_state_version: i64,
    ) -> ApplicationFuture<'_, AdminCredentialCheckResult>;
}

/// 论坛登录态不区分世界；shared 游戏 Cookie 必须分别检查两个世界。
pub fn credential_check_worlds(
    kind: AdminCredentialKind,
    world: AdminCredentialWorld,
) -> &'static [AdminCredentialWorld] {
    use AdminCredentialWorld::{Isekai, Main, Shared};
    match (kind, world) {
        (AdminCredentialKind::ForumCookie, _) => &[Shared],
        (_, Main) => &[Main],
        (_, Isekai) => &[Isekai],
        (_, Shared) => &[Main, Isekai],
    }
}
