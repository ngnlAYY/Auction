//! Admin 凭据管理端口。
//!
//! 凭据属于完整 PostgreSQL 档：Admin 只能看到不含明文和密文的安全投影，明文只在导入
//! 命令和加密适配器之间短暂流动。命令摘要、版本 fence、密文信封和删除后的命令重放
//! 都由这个端口明确建模，避免 HTTP 或 Worker 绕过凭据 owner 直接访问数据库。

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::ApplicationError;

use super::{ApplicationFuture, RequestHash};

/// Admin 凭据明文的最大 UTF-8 字节数。
pub const MAX_ADMIN_CREDENTIAL_SECRET_BYTES: usize = 16 * 1024;
/// Admin 凭据列表允许的最大页大小。
pub const MAX_ADMIN_CREDENTIAL_PAGE_SIZE: i64 = 200;
/// Admin 凭据列表允许的最大偏移。
pub const MAX_ADMIN_CREDENTIAL_OFFSET: i64 = 2_147_483_647;

/// 旧版 Admin 可管理的外部凭据类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminCredentialKind {
    MailCookie,
    ForumCookie,
}

impl AdminCredentialKind {
    /// 返回数据库、HTTP 和请求摘要使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MailCookie => "mail_cookie",
            Self::ForumCookie => "forum_cookie",
        }
    }

    /// 从外部闭集字符串解析凭据类型。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mail_cookie" => Some(Self::MailCookie),
            "forum_cookie" => Some(Self::ForumCookie),
            _ => None,
        }
    }

    /// 返回凭据在 Worker 侧的稳定命名空间前缀。
    pub const fn scope_prefix(self) -> &'static str {
        match self {
            Self::MailCookie => "mooglemail",
            Self::ForumCookie => "ehentai",
        }
    }
}

/// Admin 凭据可绑定的旧版 world 范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminCredentialWorld {
    Main,
    Isekai,
    Shared,
}

impl AdminCredentialWorld {
    /// 返回 HTTP、摘要和管理端展示使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isekai => "isekai",
            Self::Shared => "shared",
        }
    }

    /// 从外部闭集字符串解析 world 范围。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "main" => Some(Self::Main),
            "isekai" => Some(Self::Isekai),
            "shared" => Some(Self::Shared),
            _ => None,
        }
    }

    /// 将 Admin world 映射为数据库和 Worker 使用的内部 scope。
    pub fn scope(self, kind: AdminCredentialKind) -> String {
        format!("{}:{}", kind.scope_prefix(), self.as_str())
    }
}

/// 凭据当前状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminCredentialStatus {
    Active,
    Disabled,
    Invalid,
}

impl AdminCredentialStatus {
    /// 返回数据库、HTTP 和审计使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Invalid => "invalid",
        }
    }

    /// 从数据库字符串解析闭集状态。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            "invalid" => Some(Self::Invalid),
            _ => None,
        }
    }

    /// Admin API 允许的人工状态变更只包含 active/disabled；invalid 由 Worker 失败标记。
    pub const fn is_admin_selectable(self) -> bool {
        matches!(self, Self::Active | Self::Disabled)
    }
}

/// 不含密文、明文和密钥指纹的 Admin 凭据安全投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminCredential {
    pub id: Uuid,
    pub kind: AdminCredentialKind,
    pub world: AdminCredentialWorld,
    pub label: String,
    pub status: AdminCredentialStatus,
    pub state_version: i64,
    pub created_at: DateTime<Utc>,
    pub rotated_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

impl AdminCredential {
    /// 校验 storage 返回的投影，防止未知 kind、scope 或非法版本进入 HTTP。
    pub fn validate(&self) -> bool {
        !self.id.is_nil()
            && valid_text(&self.label, 256)
            && self.state_version > 0
            && valid_time(self.created_at)
            && self.rotated_at.is_none_or(valid_time)
            && valid_time(self.updated_at)
            && self.updated_at >= self.created_at
            && self.rotated_at.is_none_or(|value| value >= self.created_at)
    }
}

/// Admin 凭据列表请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCredentialListRequest {
    pub kind: Option<AdminCredentialKind>,
    pub world: Option<AdminCredentialWorld>,
    pub status: Option<AdminCredentialStatus>,
    pub limit: i64,
    pub offset: i64,
}

impl AdminCredentialListRequest {
    /// 校验闭集筛选和有界数字分页。
    pub fn validate(&self) -> bool {
        self.limit > 0
            && self.limit <= MAX_ADMIN_CREDENTIAL_PAGE_SIZE
            && self.offset >= 0
            && self.offset <= MAX_ADMIN_CREDENTIAL_OFFSET
    }
}

/// Admin 凭据列表页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCredentialPage {
    pub credentials: Vec<AdminCredential>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl AdminCredentialPage {
    /// 校验页边界、筛选命中和稳定排序。
    pub fn validate(&self, request: &AdminCredentialListRequest) -> bool {
        request.validate()
            && self.total >= 0
            && self.limit == request.limit
            && self.offset == request.offset
            && self.credentials.len() as i64 <= self.limit
            && self.credentials.iter().enumerate().all(|(index, credential)| {
                credential.validate()
                    && request.kind.is_none_or(|kind| credential.kind == kind)
                    && request.world.is_none_or(|world| credential.world == world)
                    && request.status.is_none_or(|status| credential.status == status)
                    && self.credentials.get(index + 1).is_none_or(|next| {
                        credential.created_at > next.created_at
                            || credential.created_at == next.created_at && credential.id > next.id
                    })
            })
    }
}

/// 凭据管理命令闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminCredentialCommandKind {
    Import,
    Rename,
    SetStatus,
    Delete,
}

impl AdminCredentialCommandKind {
    /// 返回命令账本与审计使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Rename => "rename",
            Self::SetStatus => "set_status",
            Self::Delete => "delete",
        }
    }
}

/// 凭据导入命令；明文只在当前命令生命周期内存在。
pub struct AdminCredentialImportCommand {
    pub command_id: Uuid,
    pub kind: AdminCredentialKind,
    pub world: AdminCredentialWorld,
    pub label: String,
    pub secret: AdminCredentialSecret,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

impl fmt::Debug for AdminCredentialImportCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminCredentialImportCommand")
            .field("command_id", &self.command_id)
            .field("kind", &self.kind)
            .field("world", &self.world)
            .field("label", &self.label)
            .field("secret", &"[REDACTED]")
            .field("actor_id", &self.actor_id)
            .field("request_id", &self.request_id)
            .field("now", &self.now)
            .finish()
    }
}

/// 凭据标签修改命令。
#[derive(Debug, Clone)]
pub struct AdminCredentialRenameCommand {
    pub command_id: Uuid,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub label: String,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// 凭据状态修改命令。
#[derive(Debug, Clone)]
pub struct AdminCredentialStatusCommand {
    pub command_id: Uuid,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub status: AdminCredentialStatus,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// 凭据删除命令。
#[derive(Debug, Clone)]
pub struct AdminCredentialDeleteCommand {
    pub command_id: Uuid,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// 由加密适配器产生的数据库密文信封。
pub struct AdminCredentialEnvelope {
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    encryption_key_id: String,
    fingerprint: String,
}

impl AdminCredentialEnvelope {
    /// 建立并校验 AES-GCM 信封；密文和 nonce 不允许进入 JSON/日志。
    pub fn new(
        ciphertext: Vec<u8>,
        nonce: Vec<u8>,
        encryption_key_id: String,
        fingerprint: String,
    ) -> Result<Self, AdminCredentialError> {
        if ciphertext.len() < 16
            || nonce.len() != 12
            || !valid_text(&encryption_key_id, 256)
            || !valid_text(&fingerprint, 256)
        {
            return Err(AdminCredentialError::StoredResultCorrupt);
        }
        Ok(Self { ciphertext, nonce, encryption_key_id, fingerprint })
    }

    /// 返回密文切片供 storage 写入。
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    /// 返回 12 字节随机 nonce。
    pub fn nonce(&self) -> &[u8] {
        &self.nonce
    }

    /// 返回数据密钥轮换标识。
    pub fn encryption_key_id(&self) -> &str {
        &self.encryption_key_id
    }

    /// 返回密文指纹。
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

impl fmt::Debug for AdminCredentialEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminCredentialEnvelope")
            .field("ciphertext", &"[REDACTED]")
            .field("nonce", &"[REDACTED]")
            .field("encryption_key_id", &self.encryption_key_id)
            .field("fingerprint", &"[REDACTED]")
            .finish()
    }
}

impl Drop for AdminCredentialEnvelope {
    fn drop(&mut self) {
        self.ciphertext.fill(0);
        self.nonce.fill(0);
    }
}

/// 凭据明文值对象；Drop 时清零内存，禁止 Debug/序列化泄漏。
pub struct AdminCredentialSecret(Vec<u8>);

impl AdminCredentialSecret {
    /// 从 HTTP 已解析的 UTF-8 字符串建立明文值对象。
    pub fn new(value: String) -> Result<Self, AdminCredentialError> {
        if value.is_empty()
            || value.len() > MAX_ADMIN_CREDENTIAL_SECRET_BYTES
            || value
                .chars()
                .any(|character| character == '\0' || character == '\r' || character == '\n')
        {
            return Err(AdminCredentialError::InvalidRequest);
        }
        Ok(Self(value.into_bytes()))
    }

    /// 返回明文只读字节，调用方不得复制到持久化或日志结构。
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for AdminCredentialSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl Drop for AdminCredentialSecret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// Admin 凭据导入写入；只含密文信封。
pub struct AdminCredentialImportWrite {
    pub command_id: Uuid,
    pub request_hash: RequestHash,
    pub credential_id: Uuid,
    pub kind: AdminCredentialKind,
    pub world: AdminCredentialWorld,
    pub label: String,
    pub envelope: AdminCredentialEnvelope,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// Admin 凭据标签/状态修改写入。
#[derive(Debug)]
pub struct AdminCredentialMutationWrite {
    pub command_id: Uuid,
    pub request_hash: RequestHash,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub action: AdminCredentialCommandKind,
    pub label: Option<String>,
    pub status: Option<AdminCredentialStatus>,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// Admin 凭据删除写入。
#[derive(Debug)]
pub struct AdminCredentialDeleteWrite {
    pub command_id: Uuid,
    pub request_hash: RequestHash,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// 删除命令的安全重放结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminCredentialDeleteResult {
    pub credential_id: Uuid,
    pub state_version: i64,
}

impl AdminCredentialDeleteResult {
    /// 校验删除结果，不依赖已删除的凭据行。
    pub fn validate(&self) -> bool {
        !self.credential_id.is_nil() && self.state_version > 1
    }
}

/// 凭据命令账本中的安全结果；删除后 `credential` 必须为空。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCredentialCommandResult {
    pub credential: Option<AdminCredential>,
    pub deleted: Option<AdminCredentialDeleteResult>,
}

/// Worker 读取的单条 active 凭据记录；仍不包含明文。
pub struct AdminCredentialRuntimeRecord {
    pub id: Uuid,
    pub kind: AdminCredentialKind,
    pub scope: String,
    pub envelope: AdminCredentialEnvelope,
    pub created_at: DateTime<Utc>,
}

impl fmt::Debug for AdminCredentialRuntimeRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminCredentialRuntimeRecord")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("scope", &self.scope)
            .field("envelope", &self.envelope)
            .field("created_at", &self.created_at)
            .finish()
    }
}

impl AdminCredentialRuntimeRecord {
    /// 校验 Worker 读取的内部 scope、密文信封和稳定排序字段。
    pub fn validate(&self) -> bool {
        !self.id.is_nil()
            && self.scope == self.scope.trim()
            && !self.scope.is_empty()
            && self.scope.starts_with(self.kind.scope_prefix())
            && self.created_at.timestamp_nanos_opt().is_some()
            && self.envelope.ciphertext().len() >= 16
            && self.envelope.nonce().len() == 12
    }
}

/// Dashboard 使用的凭据按 kind/world 聚合行，不包含密文、明文或指纹。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminCredentialHealthScope {
    pub kind: AdminCredentialKind,
    pub world: AdminCredentialWorld,
    pub active_count: i64,
    pub disabled_count: i64,
    pub invalid_count: i64,
    /// 该范围内最近一次凭据记录变化时间；没有记录时为空。
    pub updated_at: Option<DateTime<Utc>>,
}

impl AdminCredentialHealthScope {
    /// 校验计数和时间不会以负数、溢出或无来源状态进入 Admin dashboard。
    pub fn validate(&self) -> bool {
        let total = self
            .active_count
            .checked_add(self.disabled_count)
            .and_then(|count| count.checked_add(self.invalid_count));
        let time_shape_valid = match total {
            Some(0) => self.updated_at.is_none(),
            Some(value) if value > 0 => self.updated_at.is_some(),
            _ => false,
        };
        self.active_count >= 0
            && self.disabled_count >= 0
            && self.invalid_count >= 0
            && self.updated_at.is_none_or(|value| value.timestamp_nanos_opt().is_some())
            && time_shape_valid
    }
}

/// Dashboard 的凭据安全聚合快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminCredentialHealthSnapshot {
    pub observed_at: DateTime<Utc>,
    /// 凭据管理命令账本中最近一次变更时间，用于判断 Worker 是否需要重启。
    pub last_admin_change_at: Option<DateTime<Utc>>,
    pub scopes: Vec<AdminCredentialHealthScope>,
}

impl AdminCredentialHealthSnapshot {
    /// 校验稳定排序、范围唯一性和有界计数。
    pub fn validate(&self) -> bool {
        self.observed_at.timestamp_nanos_opt().is_some()
            && self.last_admin_change_at.is_none_or(|value| value.timestamp_nanos_opt().is_some())
            && self.scopes.len() <= 16
            && self.scopes.iter().enumerate().all(|(index, scope)| {
                scope.validate()
                    && self.scopes[..index].iter().all(|previous| {
                        previous.kind != scope.kind || previous.world != scope.world
                    })
            })
    }
}

impl AdminCredentialCommandResult {
    /// 校验命令结果的互斥形状和安全投影。
    pub fn validate(&self) -> bool {
        matches!((&self.credential, &self.deleted), (Some(credential), None) if credential.validate())
            || matches!((&self.credential, &self.deleted), (None, Some(deleted)) if deleted.validate())
    }
}

/// Admin 凭据业务错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdminCredentialError {
    #[error("Admin 凭据命令参数无效")]
    InvalidRequest,
    #[error("Admin 凭据不存在")]
    NotFound,
    #[error("Admin 凭据标签或密文指纹冲突")]
    Conflict,
    #[error("凭据命令 ID 与已有请求负载冲突")]
    CommandConflict,
    #[error("凭据状态版本冲突，当前版本为 {current_version}")]
    VersionConflict { current_version: i64 },
    #[error("Admin 凭据 owner 返回结果损坏")]
    StoredResultCorrupt,
    #[error("Admin 凭据加密能力不可用")]
    CryptoUnavailable,
}

/// Admin 凭据密封端口；mail/forum 使用不同的加密用途域。
pub trait AdminCredentialSealer: Send + Sync {
    fn seal(
        &self,
        kind: AdminCredentialKind,
        credential_id: Uuid,
        scope: &str,
        secret: &[u8],
    ) -> Result<AdminCredentialEnvelope, crate::ApplicationError>;
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
    fn list_active(
        &self,
        kind: AdminCredentialKind,
        world: AdminCredentialWorld,
    ) -> ApplicationFuture<'_, Vec<AdminCredentialRuntimeRecord>>;
}

/// Worker 凭据失效回写端口。
///
/// 远端明确返回登录失效时，Worker 只能把当前凭据标记为 `invalid`；该操作必须是幂等的，
/// 不能把远端写入结果重试成第二次写入，也不能通过 Admin 命令账本伪造一个用户命令。
pub trait AdminCredentialRuntimeWriter: Send + Sync {
    fn mark_invalid(&self, credential_id: Uuid, now: DateTime<Utc>) -> ApplicationFuture<'_, ()>;
}

/// Admin dashboard 的凭据聚合读取端口。
pub trait AdminCredentialHealthReader: Send + Sync {
    fn health(&self) -> ApplicationFuture<'_, AdminCredentialHealthSnapshot>;
}

/// Worker 凭据解密端口；明文只在 integrations 与远端客户端之间短暂存在。
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

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_time(value: DateTime<Utc>) -> bool {
    value.timestamp_nanos_opt().is_some()
}
